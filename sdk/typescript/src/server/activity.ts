interface DefineArgs {
  html: string;
}

export function defineActivity(_args: DefineArgs) {
  return {
    create: () => {},
    /** Typo alias kept for compatibility with existing extension code. */
    craete: (_args?: unknown) => {},
  };
}

export const activity = {
  define: defineActivity,
};
