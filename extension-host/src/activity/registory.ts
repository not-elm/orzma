import type { ActivityHost } from "./host.ts";

export class ActivityHostRegistry {
  private map = new Map<string, ActivityHost>();

  insert(activityId: string, host: ActivityHost): void {
    this.map.set(activityId, host);
  }

  get(activityId: string): ActivityHost | undefined {
    return this.map.get(activityId);
  }

  remove(activityId: string): ActivityHost | undefined {
    const h = this.map.get(activityId);
    this.map.delete(activityId);
    return h;
  }

  list(): { activityId: string; host: ActivityHost }[] {
    return [...this.map.entries()].map(([activityId, host]) => ({
      activityId,
      host,
    }));
  }

  size(): number {
    return this.map.size;
  }
}

export const activities = new ActivityHostRegistry();
