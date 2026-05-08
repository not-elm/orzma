const COMMAND_NAME_RE = /^[a-z][a-z0-9_-]{0,63}$/;

export function assertCommandName(name: string): void {
  if (!COMMAND_NAME_RE.test(name)) {
    throw new Error(`invalid command name: ${JSON.stringify(name)}`);
  }
}

export function shellSingleQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}
