interface DefineArgs<T> {
  html: string;
  initialData: T;
}

export async function createActivity<T>(args: DefineArgs<T>) {}
