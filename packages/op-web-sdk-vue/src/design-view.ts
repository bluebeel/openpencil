import { defineComponent, Fragment, h, onMounted, onUnmounted, ref, shallowRef, watch } from 'vue';
import { createViewer, type OpViewer } from '@zseven-w/op-web-sdk';
import { provideViewerRef } from './use-viewer.js';

export const DesignView = defineComponent({
  name: 'DesignView',
  props: {
    doc: {
      type: [String, Object] as unknown as () => string | Uint8Array,
      default: undefined,
    },
    wasmUrl: { type: String, default: undefined },
  },
  emits: { load: (_viewer: OpViewer) => true },
  setup(props, { emit, slots }) {
    const canvasRef = ref<HTMLCanvasElement | null>(null);
    const viewerRef = shallowRef<OpViewer | null>(null);
    provideViewerRef(viewerRef);
    let source: string | Uint8Array | undefined;
    let moduleUrl: string | undefined;
    let stop: (() => void) | undefined;

    // Reactive replacements use the existing canvas/CanvasKit session. A
    // document arriving during module initialization is loaded only once.
    watch(() => props.doc, (doc) => {
      const viewer = viewerRef.value;
      if (viewer && moduleUrl === props.wasmUrl && doc !== undefined && doc !== source) {
        viewer.load(doc);
        source = doc;
      }
    }, { flush: 'post' });

    onMounted(() => {
      stop = watch(
        [() => props.wasmUrl, () => props.doc !== undefined] as const,
        async ([wasmUrl], _old, onCleanup) => {
          const canvas = canvasRef.value;
          if (!canvas) return;
          let cancelled = false;
          let made: OpViewer | null = null;
          const abort = new AbortController();
          onCleanup(() => {
            cancelled = true;
            abort.abort();
            if (viewerRef.value === made) viewerRef.value = null;
            made?.destroy();
          });
          viewerRef.value = null;
          try {
            const viewer = await createViewer({ canvas, wasmUrl, signal: abort.signal });
            if (cancelled) {
              viewer.destroy();
              return;
            }
            made = viewer;
            source = props.doc;
            if (source !== undefined) viewer.load(source);
            moduleUrl = wasmUrl;
            viewerRef.value = viewer;
            emit('load', viewer);
          } catch (error) {
            if (cancelled) return;
            try { made?.destroy(); } catch { /* Preserve initialization failure. */ }
            viewerRef.value = null;
            // Vue routes this async watch failure through its error handlers.
            throw error;
          }
        },
        { immediate: true, flush: 'post' },
      );
    });
    onUnmounted(() => { stop?.(); });

    return () => h(Fragment, [
      h('canvas', { ref: canvasRef, style: 'width:100%;height:100%;display:block' }),
      slots.default?.(),
    ]);
  },
});
