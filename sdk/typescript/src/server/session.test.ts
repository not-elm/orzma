import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as daemonClient from './daemon-client.ts';
import { Session } from './session.ts';

let getJsonSpy: ReturnType<typeof vi.spyOn>;
let deleteNoContentSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  getJsonSpy = vi.spyOn(daemonClient, 'getJson');
  deleteNoContentSpy = vi.spyOn(daemonClient, 'deleteNoContent').mockResolvedValue();
});

afterEach(() => {
  getJsonSpy.mockRestore();
  deleteNoContentSpy.mockRestore();
});

describe('Session.fetch', () => {
  it('hits /sessions/:id and decodes linked_windows / active_window', async () => {
    getJsonSpy.mockResolvedValue({
      session_id: 's1',
      name: 'S',
      linked_windows: ['w1', 'w2'],
      active_window: 'w2',
    });
    const s = await Session.fetch('s1');
    expect(getJsonSpy).toHaveBeenCalledWith('/sessions/s1');
    expect(s.id).toBe('s1');
    expect(s.name).toBe('S');
    expect(s.linkedWindowIds).toEqual(['w1', 'w2']);
    expect(s.activeWindowId).toBe('w2');
  });
});

describe('Session.delete', () => {
  it('DELETEs /sessions/:id', async () => {
    await new Session({ id: 's1', name: 'S' }).delete();
    expect(deleteNoContentSpy).toHaveBeenCalledWith('/sessions/s1');
  });
});
