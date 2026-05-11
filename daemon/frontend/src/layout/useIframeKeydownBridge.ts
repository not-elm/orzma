import { type RefObject, useEffect } from 'react';
import { attachKeydownTarget, detachKeydownTarget } from '../shortcuts/usePrefixMode';

export function useIframeKeydownBridge(ref: RefObject<HTMLIFrameElement | null>) {
  useEffect(() => {
    const iframe = ref.current;
    if (!iframe) return;

    let attachedDoc: Document | null = null;
    const attach = () => {
      let doc: Document | null = null;
      try {
        doc = iframe.contentDocument;
      } catch {
        // cross-origin: cannot access — skip
      }
      if (!doc) {
        console.warn('useIframeKeydownBridge: iframe contentDocument unavailable (cross-origin?)');
        return;
      }
      if (attachedDoc === doc) return;
      if (attachedDoc) detachKeydownTarget(attachedDoc);
      attachKeydownTarget(doc);
      attachedDoc = doc;
    };

    iframe.addEventListener('load', attach);
    // Attach once eagerly in case the iframe has already loaded.
    attach();

    return () => {
      iframe.removeEventListener('load', attach);
      if (attachedDoc) {
        detachKeydownTarget(attachedDoc);
        attachedDoc = null;
      }
    };
  }, [ref]);
}
