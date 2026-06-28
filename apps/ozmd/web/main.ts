import 'katex/dist/katex.min.css';
import 'highlight.js/styles/github-dark.css';
import { ozma } from '@ozma/web';
import DOMPurify from 'dompurify';
import mermaid from 'mermaid';
import { installHeadingAnchors } from './anchors';
import { classifyLink } from './links';
import { renderMarkdown } from './render';
import { Search } from './search';

mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: 'dark' });

const content = document.getElementById('content') as HTMLElement;
const search = new Search();

let mermaidSeq = 0;
let renderGeneration = 0;

type ScrollTo =
  | { kind: 'preserve' }
  | { kind: 'top' }
  | { kind: 'ratio'; ratio: number }
  | { kind: 'slug'; slug: string };

interface ContentPayload {
  markdown: string;
  baseDir: string;
  scrollTo: ScrollTo;
}

function headingEls(): HTMLElement[] {
  return Array.from(content.querySelectorAll<HTMLElement>('h1,h2,h3,h4,h5,h6')).filter((h) =>
    /^h\d+$/.test(h.id),
  );
}

function scrollMax(): number {
  return document.documentElement.scrollHeight - window.innerHeight;
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
  const max = scrollMax();
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
  const max = scrollMax();
  window.scrollTo({ top: max > 0 ? anchor.ratio * max : 0 });
}

function scrollToAnchor(fragment: string): boolean {
  const el = document.getElementById(fragment);
  if (el === null) {
    return false;
  }
  el.scrollIntoView({ block: 'start' });
  reportScrollState();
  return true;
}

function applyScrollTarget(scrollTo: ScrollTo, anchor: ScrollAnchor): void {
  switch (scrollTo.kind) {
    case 'preserve':
      restoreScrollAnchor(anchor);
      break;
    case 'top':
      window.scrollTo({ top: 0 });
      break;
    case 'ratio': {
      const max = scrollMax();
      window.scrollTo({ top: max > 0 ? scrollTo.ratio * max : 0 });
      break;
    }
    case 'slug':
      if (!scrollToAnchor(scrollTo.slug)) {
        window.scrollTo({ top: 0 });
      }
      break;
  }
}

function reportScrollState(): void {
  const max = scrollMax();
  const ratio = max > 0 ? window.scrollY / max : 0;
  const heads = headingEls();
  let currentHeadingIndex: number | null = null;
  for (let i = 0; i < heads.length; i++) {
    if (heads[i].getBoundingClientRect().top <= 1) {
      currentHeadingIndex = i;
    }
  }
  ozma.emit('scrollState', { ratio, currentHeadingIndex });
}

async function renderMermaid(): Promise<void> {
  const blocks = Array.from(content.querySelectorAll('pre code.language-mermaid'));
  for (let i = 0; i < blocks.length; i++) {
    const pre = blocks[i].parentElement;
    if (pre === null) {
      continue;
    }
    try {
      const { svg } = await mermaid.render(
        `ozmd-mermaid-${mermaidSeq++}`,
        blocks[i].textContent ?? '',
      );
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
  const generation = ++renderGeneration;
  const anchor = captureScrollAnchor();
  content.innerHTML = renderMarkdown(payload.markdown);
  installHeadingAnchors(content);
  await renderMermaid();
  // NOTE: a newer setContent superseded this one during the await (rapid reloads
  // race) — skip the stale scroll so only the latest render positions the viewport.
  if (generation !== renderGeneration) {
    return;
  }
  applyScrollTarget(payload.scrollTo, anchor);
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
      window.scrollTo({ top: scrollMax() });
      break;
  }
  reportScrollState();
}

ozma.on('content', (p: ContentPayload) => {
  void setContent(p).catch(console.error);
});
ozma.on('scroll', (p: { action: string }) => {
  scrollByAction(p.action);
});
ozma.on('scrollToHeading', (p: { index: number }) => {
  document.getElementById(`h${p.index}`)?.scrollIntoView({ block: 'start' });
  reportScrollState();
});
ozma.on('search', (p: { query: string }) => {
  ozma.emit('searchCount', search.run(content, p.query));
});
ozma.on('searchNav', (p: { dir: 'next' | 'prev' }) => {
  ozma.emit('searchCount', search.navigate(p.dir));
});
ozma.on('clearSearch', () => {
  search.clear(content);
});

content.addEventListener('click', (e) => {
  const target = e.target;
  if (!(target instanceof Element)) {
    return;
  }
  const a = target.closest('a');
  if (a === null) {
    return;
  }
  const raw = a.getAttribute('href');
  if (raw === null) {
    return;
  }
  const link = classifyLink(raw);
  e.preventDefault();
  switch (link.kind) {
    case 'anchor':
      scrollToAnchor(link.fragment);
      break;
    case 'markdown':
      reportScrollState();
      ozma.emit('navigate', { path: link.path, fragment: link.fragment });
      break;
    case 'file':
      ozma.emit('openPath', { path: link.path });
      break;
    case 'external':
      ozma.emit('openExternal', { url: link.url });
      break;
    case 'ignore':
      break;
  }
});

window.addEventListener('scroll', reportScrollState, { passive: true });

void ozma
  .call<ContentPayload>('ready')
  .then((doc) => setContent(doc))
  .catch(console.error);
