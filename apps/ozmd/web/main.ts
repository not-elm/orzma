import 'katex/dist/katex.min.css';
import 'highlight.js/styles/github-dark.css';
import { ozma } from '@ozma/web';
import DOMPurify from 'dompurify';
import mermaid from 'mermaid';
import { renderMarkdown } from './render';
import { Search } from './search';

mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: 'dark' });

const content = document.getElementById('content') as HTMLElement;
const search = new Search();

interface ContentPayload {
  markdown: string;
  baseDir: string;
}

function headingEls(): HTMLElement[] {
  return Array.from(content.querySelectorAll<HTMLElement>('h1,h2,h3,h4,h5,h6')).filter((h) =>
    /^h\d+$/.test(h.id),
  );
}

interface ScrollAnchor {
  id: string | null;
  offset: number;
  ratio: number;
}

function captureScrollAnchor(): ScrollAnchor {
  const heads = headingEls();
  let id: string | null = null;
  let offset = 0;
  for (const h of heads) {
    const top = h.getBoundingClientRect().top;
    if (top <= 1) {
      id = h.id;
      offset = top;
    } else {
      break;
    }
  }
  const max = document.documentElement.scrollHeight - window.innerHeight;
  const ratio = max > 0 ? window.scrollY / max : 0;
  return { id, offset, ratio };
}

function restoreScrollAnchor(anchor: ScrollAnchor): void {
  if (anchor.id !== null) {
    const el = document.getElementById(anchor.id);
    if (el !== null) {
      window.scrollTo({ top: el.getBoundingClientRect().top + window.scrollY - anchor.offset });
      return;
    }
  }
  const max = document.documentElement.scrollHeight - window.innerHeight;
  window.scrollTo({ top: max > 0 ? anchor.ratio * max : 0 });
}

function reportScrollState(): void {
  const max = document.documentElement.scrollHeight - window.innerHeight;
  const ratio = max > 0 ? window.scrollY / max : 0;
  const heads = headingEls();
  let currentHeadingIndex: number | null = null;
  for (let i = 0; i < heads.length; i++) {
    if (heads[i].getBoundingClientRect().top <= 1) {
      currentHeadingIndex = i;
    }
  }
  void ozma.call('scrollState', { ratio, currentHeadingIndex });
}

async function renderMermaid(): Promise<void> {
  const blocks = Array.from(content.querySelectorAll('pre code.language-mermaid'));
  for (let i = 0; i < blocks.length; i++) {
    const pre = blocks[i].parentElement;
    if (pre === null) {
      continue;
    }
    try {
      const { svg } = await mermaid.render(`ozmd-mermaid-${i}`, blocks[i].textContent ?? '');
      // NOTE: mermaid source is attacker-controllable; strict mode sanitizes, and
      // this DOMPurify pass (allowing SVG foreignObject) is defense-in-depth.
      pre.outerHTML = DOMPurify.sanitize(svg, {
        USE_PROFILES: { svg: true, svgFilters: true, html: true },
        ADD_TAGS: ['foreignObject'],
      });
    } catch {
      // NOTE: a malformed diagram must not abort the whole render — leave the
      // original fenced code block visible as the fallback.
    }
  }
}

async function setContent(payload: ContentPayload): Promise<void> {
  const anchor = captureScrollAnchor();
  content.innerHTML = renderMarkdown(payload.markdown);
  await renderMermaid();
  restoreScrollAnchor(anchor);
  reportScrollState();
}

function scrollByAction(action: string): void {
  const page = window.innerHeight;
  const line = 60;
  switch (action) {
    case 'down':
      window.scrollBy({ top: line });
      break;
    case 'up':
      window.scrollBy({ top: -line });
      break;
    case 'halfDown':
      window.scrollBy({ top: page / 2 });
      break;
    case 'halfUp':
      window.scrollBy({ top: -page / 2 });
      break;
    case 'pageDown':
      window.scrollBy({ top: page });
      break;
    case 'pageUp':
      window.scrollBy({ top: -page });
      break;
    case 'top':
      window.scrollTo({ top: 0 });
      break;
    case 'bottom':
      window.scrollTo({ top: document.documentElement.scrollHeight });
      break;
  }
  reportScrollState();
}

ozma.on('content', (p) => {
  void setContent(p as ContentPayload).catch(console.error);
});
ozma.on('scroll', (p) => {
  scrollByAction((p as { action: string }).action);
});
ozma.on('scrollToHeading', (p) => {
  const { index } = p as { index: number };
  document.getElementById(`h${index}`)?.scrollIntoView({ block: 'start' });
  reportScrollState();
});
ozma.on('search', (p) => {
  const { query } = p as { query: string };
  void ozma.call('searchCount', search.run(content, query));
});
ozma.on('searchNav', (p) => {
  const { dir } = p as { dir: 'next' | 'prev' };
  void ozma.call('searchCount', search.navigate(dir));
});
ozma.on('clearSearch', () => {
  search.clear(content);
});

window.addEventListener('scroll', reportScrollState, { passive: true });

void ozma
  .call<ContentPayload>('ready')
  .then((doc) => setContent(doc))
  .catch(console.error);
