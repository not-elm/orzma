import DOMPurify from 'dompurify';
import hljs from 'highlight.js';
import { Marked } from 'marked';
import markedAlert from 'marked-alert';
import { markedHighlight } from 'marked-highlight';
import markedKatex from 'marked-katex-extension';

let headingIndex = 0;

const marked = new Marked(
  markedHighlight({
    emptyLangClass: 'hljs',
    langPrefix: 'hljs language-',
    highlight(code, lang) {
      const language = hljs.getLanguage(lang) ? lang : 'plaintext';
      return hljs.highlight(code, { language }).value;
    },
  }),
  markedKatex({ throwOnError: false, output: 'html' }),
);

marked.use(markedAlert());

marked.use({
  renderer: {
    heading(text: string, level: number): string {
      const id = `h${headingIndex++}`;
      return `<h${level} id="${id}">${text}</h${level}>\n`;
    },
  },
});

/** Renders Markdown to sanitized HTML with sequential `id="h{n}"` heading anchors. */
export function renderMarkdown(source: string): string {
  // NOTE: rendering is synchronous and single-threaded, so resetting this
  // module-level counter at the start of each call is reentrancy-safe.
  headingIndex = 0;
  // NOTE: marked 12's Marked#parse has no async:false overload, so its return type
  // is string|Promise<string>; passing async:false guarantees a synchronous string.
  // Keep this synchronous — do not await it.
  const raw = marked.parse(source, { async: false }) as string;
  return DOMPurify.sanitize(raw);
}
