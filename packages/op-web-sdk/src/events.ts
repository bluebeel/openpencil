// Typed event emitter for OpViewer lifecycle and viewport change events.

/** Events emitted by OpViewer. */
export type ViewerEvent = 'load' | 'viewportchange';

/** Minimal typed pub/sub emitter used internally by OpViewer. */
export class Emitter {
  private map = new Map<ViewerEvent, Set<() => void>>();

  /** Subscribe to an event. Returns an unsubscribe function. */
  on(e: ViewerEvent, cb: () => void): () => void {
    let set = this.map.get(e);
    if (!set) { set = new Set(); this.map.set(e, set); }
    set.add(cb);
    return () => this.off(e, cb);
  }

  /** Unsubscribe a specific callback from an event. */
  off(e: ViewerEvent, cb: () => void): void {
    const listeners = this.map.get(e);
    if (listeners?.delete(cb) && listeners.size === 0) this.map.delete(e);
  }

  /** Fire current listeners once; callbacks may unsubscribe or subscribe. */
  emit(e: ViewerEvent): void {
    const listeners = this.map.get(e);
    if (!listeners?.size) return;
    // Iterating the live Set can visit callbacks added by a callback forever.
    // Snapshot identity only; removed/destroyed subscriptions stay removed.
    for (const cb of Array.from(listeners)) {
      if (this.map.get(e) === listeners && listeners.has(cb)) cb();
    }
  }

  /** Remove all listeners for all events. */
  clear(): void { this.map.clear(); }
}
