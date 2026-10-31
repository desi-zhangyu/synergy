import { z } from "zod"
import { RuntimeContext } from "../lifecycle/context"
import { Storage } from "./storage"
import { StorageIntegrityError } from "./errors"
import type { StoredEvent, StoredRecord, StoreTransaction, TransactionalStore } from "./transactional-store"

export namespace StorageEventSinks {
  const DeliverySchema = z
    .object({
      version: z.literal(1),
      sinkID: z.string().min(1),
      eventID: z.string().min(1),
      partition: z.string().min(1).max(256),
      sequence: z.number().int().positive().max(Number.MAX_SAFE_INTEGER),
      payload: z.unknown(),
    })
    .strict()
  export type Delivery = z.infer<typeof DeliverySchema>
  export type Projection = { partition: string; payload: unknown; eventID?: string }
  export interface Sink {
    id: string
    /** Ordered projections commit atomically with their source fact. External effects are forbidden. */
    capture(
      event: Readonly<StoredEvent>,
    ): Projection | Projection[] | undefined | Promise<Projection | Projection[] | undefined>
    /** Optional transaction-local host receipt after sequence allocation; no external effects. */
    captured?(delivery: Readonly<Delivery>): void | Promise<void>
    /** Acknowledges durable acceptance. Receivers deduplicate by eventID and sinkID. */
    deliver(delivery: Readonly<Delivery>): Promise<void>
  }
  const state = RuntimeContext.state(() => ({
    sinks: new Map<string, Sink>(),
    flushing: new WeakMap<TransactionalStore, Promise<unknown>>(),
    accepted: new WeakMap<TransactionalStore, Map<string, StoredRecord<Delivery>>>(),
    cleaning: new WeakMap<TransactionalStore, Promise<void>>(),
  }))

  export function register(sink: Sink) {
    RuntimeContext.assertCompositionOpen("storage event sinks")
    if (!/^[a-zA-Z0-9._-]{1,128}$/.test(sink.id)) throw new Error("Invalid storage event sink identity")
    const existing = state().sinks.get(sink.id)
    if (existing === sink) return
    if (existing) throw new Error(`Storage event sink ${sink.id} is already registered`)
    state().sinks.set(sink.id, sink)
  }

  /** Called by Storage.enqueue; the external queue is independent of Runtime Bus epochs. */
  export async function capture(tx: StoreTransaction, event: StoredEvent) {
    for (const sink of state().sinks.values()) {
      const selected = await sink.capture(structuredClone(event))
      if (selected === undefined) continue
      for (const projection of Array.isArray(selected) ? selected : [selected]) {
        const partition = z.string().min(1).max(256).parse(projection.partition)
        const meta = ["event_delivery_meta", sink.id, partition]
        const [previous] = await tx.readMany<{ sequence: number }>([meta])
        const sequence = (previous?.sequence ?? 0) + 1
        const delivery = DeliverySchema.parse({
          version: 1,
          sinkID: sink.id,
          eventID: projection.eventID ?? event.id,
          partition,
          sequence,
          payload: structuredClone(projection.payload),
        })
        await tx.write(meta, { sequence })
        await tx.write(key(delivery), delivery)
        await sink.captured?.(structuredClone(delivery))
      }
    }
  }

  /** Hosts may defer bounded local cleanup after the sink acknowledges acceptance. */
  export async function flush(
    options: { limit?: number; deferCleanup?: boolean } = {},
  ): Promise<{ delivered: number }> {
    if (Storage.inTransaction()) throw new Error("Flush storage event sinks outside a transaction")
    const limit = z
      .number()
      .int()
      .min(1)
      .max(1024)
      .parse(options.limit ?? 128)
    const { store } = Storage.current()
    const current = state()
    const pending = (current.flushing.get(store) ?? Promise.resolve())
      .catch(() => {})
      .then(async () => {
        const accepted = current.accepted.get(store) ?? new Map<string, StoredRecord<Delivery>>()
        current.accepted.set(store, accepted)
        if (!options.deferCleanup || accepted.size >= 1024) await cleanup(store, accepted)
        const alreadyAccepted = new Set(accepted.keys())
        let after: string[] | undefined
        let delivered = 0
        try {
          while (delivered < limit) {
            const records = await Storage.query<Delivery>({
              kind: "event_delivery",
              limit: Math.min(limit - delivered, 1024),
              after,
            })
            if (!records.length) break
            for (const record of records) {
              after = record.key
              const identity = JSON.stringify(record.key)
              if (alreadyAccepted.has(identity) || accepted.has(identity)) continue
              const delivery = DeliverySchema.parse(record.value)
              if (JSON.stringify(key(delivery)) !== identity)
                throw new StorageIntegrityError("External event delivery identity does not match its record")
              const sink = current.sinks.get(delivery.sinkID)
              if (!sink) throw new StorageIntegrityError(`Storage event sink ${delivery.sinkID} is not registered`)
              await sink.deliver(structuredClone(delivery))
              accepted.set(identity, record)
              delivered++
            }
          }
        } finally {
          if (options.deferCleanup) void cleanup(store, accepted).catch(() => {})
          else await cleanup(store, accepted)
        }
        return { delivered }
      })
    current.flushing.set(store, pending)
    try {
      return await pending
    } finally {
      if (current.flushing.get(store) === pending) current.flushing.delete(store)
    }
  }

  /** Accepted records awaiting local cleanup do not count as transport backlog. */
  export async function hasPending(): Promise<boolean> {
    const { store } = Storage.current()
    const accepted = new Set(state().accepted.get(store)?.keys())
    let after: string[] | undefined
    for (;;) {
      const records = await Storage.query<Delivery>({ kind: "event_delivery", limit: 128, after })
      if (!records.length) return false
      if (records.some((record) => !accepted.has(JSON.stringify(record.key)))) return true
      after = records.at(-1)!.key
    }
  }

  function cleanup(store: TransactionalStore, accepted: Map<string, StoredRecord<Delivery>>): Promise<void> {
    const current = state()
    const running = current.cleaning.get(store)
    if (running) return running.then(() => cleanup(store, accepted))
    const batch = [...accepted.values()].slice(0, 128)
    if (!batch.length) return Promise.resolve()
    const keys = batch.map((record) => record.key)
    const task = Storage.transaction(
      async (tx) => {
        const latest = await tx.revisions(keys)
        for (const [index, record] of batch.entries()) {
          if (latest[index] !== record.revision)
            throw new StorageIntegrityError("External event delivery changed while awaiting acknowledgment")
        }
        await tx.removeMany(keys)
      },
      { priority: "background" },
    )
      .then(() => {
        for (const record of batch) accepted.delete(JSON.stringify(record.key))
      })
      .finally(() => {
        current.cleaning.delete(store)
      })
    current.cleaning.set(store, task)
    return task.then(() => cleanup(store, accepted))
  }

  function key(delivery: Delivery) {
    return ["event_delivery", delivery.sinkID, delivery.partition, String(delivery.sequence).padStart(16, "0")]
  }
}
