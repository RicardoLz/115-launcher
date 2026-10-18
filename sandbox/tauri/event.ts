type Handler<T> = (event: { event: string; payload: T }) => void;

const subs = new Map<string, Set<Handler<unknown>>>();

export async function listen<T>(event: string, handler: Handler<T>): Promise<() => void> {
  let set = subs.get(event);
  if (!set) {
    set = new Set();
    subs.set(event, set);
  }
  const bucket = set;
  const wrapped = handler as Handler<unknown>;
  bucket.add(wrapped);
  return () => bucket.delete(wrapped);
}

export function emitTo<T>(event: string, payload: T): void {
  const set = subs.get(event);
  if (!set) return;
  for (const handler of Array.from(set)) handler({ event, payload });
}

export function listenerCount(event: string): number {
  return subs.get(event)?.size ?? 0;
}
