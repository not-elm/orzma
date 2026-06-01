import { useEffect, useRef } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeHighlight from 'rehype-highlight';
import remarkGfm from 'remark-gfm';
import { handleKey, type KeyState } from './navigation.ts';

/** Renders Markdown and binds the vim-style scroll keys to its scroll container. */
export function Preview({ markdown }: { markdown: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const keyState = useRef<KeyState>({ lastGAt: 0 });

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    const onKey = (e: KeyboardEvent) => {
      const target = handleKey(
        { key: e.key, ctrlKey: e.ctrlKey },
        { scrollTop: el.scrollTop, scrollLeft: el.scrollLeft, clientHeight: el.clientHeight },
        keyState.current,
        performance.now(),
        el.scrollHeight - el.clientHeight,
      );
      if (!target) return;
      e.preventDefault();
      if (target.top !== undefined) el.scrollTop = target.top;
      if (target.left !== undefined) el.scrollLeft = target.left;
    };
    el.addEventListener('keydown', onKey);
    return () => el.removeEventListener('keydown', onKey);
  }, []);

  return (
    // biome-ignore lint/a11y/noNoninteractiveTabindex: scroll container requires keyboard focus for vim-style navigation keys
    <div ref={ref} className="md-scroll" tabIndex={0}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeHighlight]}>
        {markdown}
      </ReactMarkdown>
    </div>
  );
}
