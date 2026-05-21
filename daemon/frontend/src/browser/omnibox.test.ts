import { describe, expect, it } from 'vitest';
import { DEFAULT_SEARCH_TEMPLATE, resolveOmniboxInput } from './omnibox';

const CUSTOM_TEMPLATE = 'https://example.com/?q={query}';

describe('resolveOmniboxInput', () => {
  describe('URL passthrough', () => {
    it('keeps fully-qualified https URLs intact', () => {
      expect(resolveOmniboxInput('https://example.com/path?a=1')).toBe(
        'https://example.com/path?a=1',
      );
    });

    it('keeps other registered schemes intact', () => {
      expect(resolveOmniboxInput('http://example.com')).toBe('http://example.com');
      expect(resolveOmniboxInput('ftp://example.com')).toBe('ftp://example.com');
    });

    it.each([
      'about:blank',
      'data:text/html,hi',
      'chrome://flags',
      'file:///tmp/a',
      'view-source:https://x.com',
    ])('keeps "%s" intact', (input) => {
      expect(resolveOmniboxInput(input)).toBe(input);
    });
  });

  describe('URL inference (no scheme)', () => {
    it('prepends https:// for dotted domains', () => {
      expect(resolveOmniboxInput('example.com')).toBe('https://example.com');
      expect(resolveOmniboxInput('example.com/path')).toBe('https://example.com/path');
    });

    it('accepts localhost as a host', () => {
      expect(resolveOmniboxInput('localhost')).toBe('https://localhost');
      expect(resolveOmniboxInput('localhost:3000')).toBe('https://localhost:3000');
      expect(resolveOmniboxInput('localhost:3000/api')).toBe('https://localhost:3000/api');
    });

    it('accepts IPv4 literals', () => {
      expect(resolveOmniboxInput('127.0.0.1')).toBe('https://127.0.0.1');
      expect(resolveOmniboxInput('127.0.0.1:8080/x')).toBe('https://127.0.0.1:8080/x');
    });

    it('accepts host:port combos', () => {
      expect(resolveOmniboxInput('example.com:8080')).toBe('https://example.com:8080');
    });

    it('rejects single bare word as host (treated as search)', () => {
      expect(resolveOmniboxInput('hello')).toBe(
        `https://duckduckgo.com/?q=${encodeURIComponent('hello')}`,
      );
    });

    it('rejects unknown TLDs that look like numbers', () => {
      expect(resolveOmniboxInput('foo.123')).toBe(
        `https://duckduckgo.com/?q=${encodeURIComponent('foo.123')}`,
      );
    });
  });

  describe('search routing', () => {
    it('routes whitespace-containing input to search', () => {
      expect(resolveOmniboxInput('hello world')).toBe(
        `https://duckduckgo.com/?q=${encodeURIComponent('hello world')}`,
      );
    });

    it('routes "claude code" to search', () => {
      expect(resolveOmniboxInput('claude code')).toBe(
        `https://duckduckgo.com/?q=${encodeURIComponent('claude code')}`,
      );
    });

    it('treats leading "?" as an explicit search', () => {
      expect(resolveOmniboxInput('?example.com')).toBe(
        `https://duckduckgo.com/?q=${encodeURIComponent('example.com')}`,
      );
    });

    it('routes quoted phrases to search', () => {
      expect(resolveOmniboxInput('"exact phrase"')).toBe(
        `https://duckduckgo.com/?q=${encodeURIComponent('"exact phrase"')}`,
      );
    });

    it('routes space-before-dot to search (Firefox rule)', () => {
      expect(resolveOmniboxInput('foo bar.com')).toBe(
        `https://duckduckgo.com/?q=${encodeURIComponent('foo bar.com')}`,
      );
    });

    it('uses a custom template when provided', () => {
      expect(resolveOmniboxInput('hello world', CUSTOM_TEMPLATE)).toBe(
        `https://example.com/?q=${encodeURIComponent('hello world')}`,
      );
    });
  });

  describe('edge cases', () => {
    it('returns empty string for empty input', () => {
      expect(resolveOmniboxInput('')).toBe('');
      expect(resolveOmniboxInput('   ')).toBe('');
    });

    it('trims leading and trailing whitespace before deciding', () => {
      expect(resolveOmniboxInput('  example.com  ')).toBe('https://example.com');
    });

    it('encodes special characters in search queries', () => {
      const result = resolveOmniboxInput('a&b=c');
      expect(result).toBe(`https://duckduckgo.com/?q=${encodeURIComponent('a&b=c')}`);
    });

    it('exposes the default template constant', () => {
      expect(DEFAULT_SEARCH_TEMPLATE).toContain('{query}');
      expect(DEFAULT_SEARCH_TEMPLATE.startsWith('https://')).toBe(true);
    });
  });
});
