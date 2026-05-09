interface DefineArgs<T> {
  html: string;
  initialData: T;
}

export async function createActivity<T>(_args: DefineArgs<T>) {}
