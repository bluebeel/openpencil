import { shallowRef, watch, type ShallowRef } from 'vue';
import type { OpViewer, PenDocument } from '@zseven-w/op-web-sdk';
import { useViewer } from './use-viewer.js';

type ViewerEvent = 'load' | 'viewportchange';

/**
 * Watch the viewer ref and keep an output ref in sync with a viewer property.
 * When the viewer becomes available (or changes), subscribe to `event` and
 * read the initial value immediately. The previous subscription is cleaned up
 * via watch's onCleanup callback.
 */
function useViewerRef<T>(
  event: ViewerEvent,
  read: (v: OpViewer) => T,
  fallback: T,
): Readonly<ShallowRef<T>> {
  const viewerRef = useViewer();
  const out = shallowRef<T>(fallback);
  watch(
    viewerRef,
    (v, _old, onCleanup) => {
      if (!v) {
        out.value = fallback;
        return;
      }
      // One initial snapshot and one notification per actual value change.
      out.value = read(v);
      onCleanup(v.on(event, () => { out.value = read(v); }));
    },
    { immediate: true },
  );

  return out;
}

export function useDocument(): Readonly<ShallowRef<PenDocument | null>> {
  return useViewerRef('load', (v) => v.document, null);
}

export function useViewport() {
  return useViewerRef('viewportchange', (v) => v.viewport, null);
}

export function useActivePage(): Readonly<ShallowRef<number>> {
  return useViewerRef('load', (v) => v.activePage, 0);
}
