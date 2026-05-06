export class ActivityHost {
  private constructor(private readonly id: string) {}
  static async create(): Promise<ActivityHost> {}
}
