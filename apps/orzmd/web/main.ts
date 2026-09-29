import 'katex/dist/katex.min.css';
import 'highlight.js/styles/github-dark.css';
import { orzma } from '@orzma/web';
import DOMPurify from 'dompurify';
import mermaid from 'mermaid';
import { installHeadingAnchors } from './anchors';
import { breadcrumb, type Chrome, type HeadingInfo, renderRail, renderToast } from './chrome';
import { FindBox } from './find';
import { HeadingTracker } from './headings';
import { collectLocalImages } from './images';
import { applyLayoutVars, FIND_CLEARANCE, RAIL_HEIGHT, reachedTop } from './layout';
import { classifyLink } from './links';
import { OutlinePanel } from './outline';
import { renderMarkdown } from './render';
import { CssHighlightPainter, measureTop, revealRange, Search } from './search';

mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: 'dark' });

/** A report to the controller (`page` event), tagged by `kind`. */
type PageEvent =
  | { kind: 'scrollState'; ratio: number; currentHeadingIndex: number | null; headingCount: number }
  | { kind: 'outlineJump'; index: number };

function emitPage(event: PageEvent): void {
  orzma.emit('page', event);
}

const content = document.getElementById('content') as HTMLElement;
const search = new Search(new CssHighlightPainter(), measureTop);

const findBox = new FindBox(document.getElementById('find') as HTMLElement, search, {
  content,
  schedule: (task) => {
    requestAnimationFrame(task);
  },
  reveal: (range) => revealRange(range, { top: FIND_CLEARANCE, bottom: window.innerHeight }),
  scrollY: () => window.scrollY,
  scrollTo: (y) => window.scrollTo({ top: y }),
});

const outlinePanel = new OutlinePanel(document.getElementById('outline') as HTMLElement, (index) =>
  emitPage({ kind: 'outlineJump', index }),
);

function applyOutline(open: boolean): void {
  if (document.body.classList.contains('outline-open') === open) {
    return;
  }
  const anchor = captureScrollAnchor();
  document.body.classList.toggle('outline-open', open);
  outlinePanel.setOpen(open);
  restoreScrollAnchor(anchor);
}

const rail = document.getElementById('rail') as HTMLElement;
const toast = document.getElementById('toast') as HTMLElement;

let chrome: Chrome | null = null;
let currentHeading: number | null = null;
let headings: HeadingInfo[] = [];
const headingTracker = new HeadingTracker();

applyLayoutVars(document.documentElement);

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
  navigated: boolean;
}

function headingEls(): HTMLElement[] {
  return Array.from(content.querySelectorAll<HTMLElement>('h1,h2,h3,h4,h5,h6')).filter((h) =>
    /^h\d+$/.test(h.id),
  );
}

function scrollMax(): number {
  return document.documentElement.scrollHeight - window.innerHeight;
}

function headingInfos(): HeadingInfo[] {
  return headingEls().map((h) => ({
    level: Number(h.tagName.slice(1)),
    text: h.textContent ?? '',
  }));
}

function renderChromeUi(): void {
  if (chrome === null) {
    return;
  }
  renderRail(rail, chrome, breadcrumb(headings, currentHeading));
  renderToast(toast, chrome.toast);
  applyOutline(chrome.outline.open);
  outlinePanel.mark(chrome.outline.selected, currentHeading);
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
    if (reachedTop(top)) {
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

function jumpTo(target: HTMLElement): void {
  target.scrollIntoView({ block: 'start' });
  const index = target.closest('h1,h2,h3,h4,h5,h6')?.id.match(/^h(\d+)$/)?.[1];
  if (index !== undefined) {
    headingTracker.jumped(Number(index), window.scrollY);
  }
  reportScrollState();
}

function scrollToAnchor(fragment: string): boolean {
  const el = document.getElementById(fragment);
  if (el === null) {
    return false;
  }
  jumpTo(el);
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
  const currentHeadingIndex = headingTracker.current(
    heads.map((h) => h.getBoundingClientRect().top),
    window.scrollY,
  );
  emitPage({ kind: 'scrollState', ratio, currentHeadingIndex, headingCount: heads.length });
  if (currentHeadingIndex !== currentHeading) {
    currentHeading = currentHeadingIndex;
    renderChromeUi();
  }
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
        `orzmd-mermaid-${mermaidSeq++}`,
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

async function stageLocalImages(root: HTMLElement): Promise<void> {
  const found = collectLocalImages(root);
  if (found.length === 0) {
    return;
  }
  const paths = [...new Set(found.map((f) => f.path))];
  let urls: (string | null)[];
  try {
    const res = await orzma.call<{ urls: (string | null)[] }, { paths: string[] }>('stageAssets', {
      paths,
    });
    urls = res.urls;
  } catch (e) {
    console.error(e);
    return;
  }
  const byPath = new Map<string, string>();
  paths.forEach((p, i) => {
    const u = urls[i];
    if (u != null) {
      byPath.set(p, u);
    }
  });
  const decoded: Promise<unknown>[] = [];
  for (const { el, path } of found) {
    const url = byPath.get(path);
    if (url !== undefined) {
      el.setAttribute('src', url);
      decoded.push(el.decode().catch(() => {}));
    }
  }
  await Promise.all(decoded);
}

async function setContent(payload: ContentPayload): Promise<void> {
  const generation = ++renderGeneration;
  if (payload.navigated) {
    findBox.reset();
  }
  const anchor = captureScrollAnchor();
  content.innerHTML = renderMarkdown(payload.markdown);
  installHeadingAnchors(content);
  headings = headingInfos();
  outlinePanel.setItems(headings);
  await renderMermaid();
  await stageLocalImages(content);
  // NOTE: a newer setContent superseded this one during the await (rapid reloads
  // race) — skip the stale scroll so only the latest render positions the viewport.
  if (generation !== renderGeneration) {
    return;
  }
  applyScrollTarget(payload.scrollTo, anchor);
  findBox.rerun();
  reportScrollState();
  renderChromeUi();
}

function scrollByAction(action: string): void {
  const page = window.innerHeight - RAIL_HEIGHT;
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

orzma.on('content', (p: ContentPayload) => {
  void setContent(p).catch(console.error);
});
orzma.on('scroll', (p: { action: string }) => {
  scrollByAction(p.action);
});
orzma.on('scrollToHeading', (p: { index: number }) => {
  const heading = document.getElementById(`h${p.index}`);
  if (heading !== null) {
    jumpTo(heading);
  }
});

orzma.on('chrome', (c: Chrome) => {
  chrome = c;
  renderChromeUi();
});

window.addEventListener('resize', renderChromeUi);

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
      orzma.emit('navigate', { path: link.path, fragment: link.fragment });
      break;
    case 'file':
      orzma.emit('openPath', { path: link.path });
      break;
    case 'external':
      orzma.emit('openExternal', { url: link.url });
      break;
    case 'ignore':
      break;
  }
});

window.addEventListener('scroll', reportScrollState, { passive: true });

void orzma
  .call<ContentPayload>('ready')
  .then((doc) => setContent(doc))
  .catch(console.error);
