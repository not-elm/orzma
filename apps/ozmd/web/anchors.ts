import GithubSlugger from 'github-slugger';

/**
 * Prepends a zero-width `<span class="ozmd-anchor" id="{slug}">` to every heading
 * under `root`, so `#section` links resolve. Slugs use GitHub's exact algorithm
 * (deduped). The heading's own `id="h{n}"` is left intact — those ids are what
 * the scroll-state reporting and outline jump depend on.
 */
export function installHeadingAnchors(root: HTMLElement): void {
  const slugger = new GithubSlugger();
  for (const heading of root.querySelectorAll<HTMLElement>('h1,h2,h3,h4,h5,h6')) {
    const slug = slugger.slug(heading.textContent ?? '');
    if (slug.length === 0) {
      continue;
    }
    const anchor = document.createElement('span');
    anchor.className = 'ozmd-anchor';
    anchor.id = slug;
    heading.prepend(anchor);
  }
}
