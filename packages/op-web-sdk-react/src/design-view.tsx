import { createElement, useEffect, useRef, useState, type ReactNode } from 'react';
import { createViewer, type OpViewer } from '@zseven-w/op-web-sdk';
import { DesignProvider } from './use-viewer.js';

export interface DesignViewProps {
  /** Serialized document JSON string or raw bytes to load on mount. */
  doc?: string | Uint8Array;
  /** URL to the WASM bundle; omit to use the bundled default. */
  wasmUrl?: string;
  /** Called once after the viewer is created and ready. */
  onLoad?: (viewer: OpViewer) => void;
  /** Optional CSS class applied to the wrapper div. */
  className?: string;
  children?: ReactNode;
}

interface ReadyViewer {
  viewer: OpViewer;
  wasmUrl?: string;
  hasDocument: boolean;
}

/** Mount one viewer per canvas/module. Document replacements reuse its renderer. */
export function DesignView(props: DesignViewProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const latest = useRef(props);
  latest.current = props;
  const live = useRef<{ viewer: OpViewer; source: DesignViewProps['doc'] } | null>(null);
  const [ready, setReady] = useState<ReadyViewer | null>(null);
  const [failure, setFailure] = useState<{ error: unknown } | null>(null);
  const hasDocument = props.doc !== undefined;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let cancelled = false;
    let made: OpViewer | null = null;
    const abort = new AbortController();
    setReady(null);
    setFailure(null);

    // Read the latest document after async initialization. Superseded props
    // never force a parse/layout of a large document that cannot be displayed.
    createViewer({ canvas, wasmUrl: props.wasmUrl, signal: abort.signal })
      .then((viewer) => {
        if (cancelled) {
          viewer.destroy();
          return;
        }
        made = viewer;
        const source = latest.current.doc;
        if (source !== undefined) viewer.load(source);
        live.current = { viewer, source };
        setReady({ viewer, wasmUrl: props.wasmUrl, hasDocument });
        latest.current.onLoad?.(viewer);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        if (live.current?.viewer === made) live.current = null;
        try { made?.destroy(); } catch { /* Preserve initialization failure. */ }
        setReady(null);
        setFailure({ error });
      });

    return () => {
      cancelled = true;
      abort.abort();
      if (live.current?.viewer === made) live.current = null;
      made?.destroy();
    };
  }, [props.wasmUrl, hasDocument]);

  useEffect(() => {
    const current = live.current;
    if (current && current.viewer === ready?.viewer && props.doc !== undefined && current.source !== props.doc) {
      current.viewer.load(props.doc);
      current.source = props.doc;
    }
  }, [props.doc, ready]);

  if (failure) throw failure.error;
  const viewer = ready?.wasmUrl === props.wasmUrl && ready?.hasDocument === hasDocument
    ? ready?.viewer
    : null;
  return createElement(
    'div',
    { className: props.className },
    createElement('canvas', { ref: canvasRef, style: { width: '100%', height: '100%', display: 'block' } }),
    viewer ? createElement(DesignProvider, { viewer }, props.children) : null,
  );
}
