interface DefineArgs {
  html: string;
}

export function defineActivity(args: DefineArgs) {
  return {
    create: () => {},
  };
}
