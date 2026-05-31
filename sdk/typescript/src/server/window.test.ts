import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as daemonClient from './daemon-client.ts';
import { Window } from './window.ts';

let getJsonSpy: ReturnType<typeof vi.spyOn>;
let postNoContentSpy: ReturnType<typeof vi.spyOn>;
let deleteNoContentSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  getJsonSpy = vi.spyOn(daemonClient, 'getJson');
  postNoContentSpy = vi.spyOn(daemonClient, 'postNoContent').mockResolvedValue();
  deleteNoContentSpy = vi.spyOn(daemonClient, 'deleteNoContent').mockResolvedValue();
});

afterEach(() => {
  getJsonSpy.mockRestore();
  postNoContentSpy.mockRestore();
  deleteNoContentSpy.mockRestore();
});

describe('Window.fetch', () => {
  it('hits /windows/:id and reifies a Window', async () => {
    getJsonSpy.mockResolvedValue({ window_id: 'w1', name: 'Win' });
    const w = await Window.fetch('w1');
    expect(getJsonSpy).toHaveBeenCalledWith('/windows/w1');
    expect(w.id).toBe('w1');
    expect(w.name).toBe('Win');
  });
});

describe('Window methods', () => {
  it('select POSTs /windows/:id/select', async () => {
    await new Window({ id: 'w1', name: 'x' }).select();
    expect(postNoContentSpy).toHaveBeenCalledWith('/windows/w1/select', {});
  });

  it('delete DELETEs /windows/:id', async () => {
    await new Window({ id: 'w1', name: 'x' }).delete();
    expect(deleteNoContentSpy).toHaveBeenCalledWith('/windows/w1');
  });
});
