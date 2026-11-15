// Bash tool timeout policy for dispatched Pi sessions.
//
// Loaded explicitly with `--extension <absolute path>` while the dispatcher keeps
// `--no-extensions`/`--no-mcp`. It only patches the built-in `bash` tool input:
//   * a missing `timeout` becomes DEFAULT_TIMEOUT_SECONDS;
//   * an explicit finite positive value is preserved, and clamped to MAX_TIMEOUT_SECONDS;
//   * any other provided value blocks the call instead of executing unbounded.
// It does not limit read/write/edit/MCP/model calls and does not parse commands.

export const DEFAULT_TIMEOUT_SECONDS = 600;
export const MAX_TIMEOUT_SECONDS = 10800;

export class BashTimeoutPolicyError extends Error {}

// Mutates the live tool input in place, mirroring what Pi executes after `tool_call`.
export function applyBashTimeoutPolicy(input) {
  if (input === null || typeof input !== 'object' || Array.isArray(input)) {
    throw new BashTimeoutPolicyError('bash input is not an object; refusing to run it without a timeout');
  }
  const provided = input.timeout;
  if (provided === undefined) {
    input.timeout = DEFAULT_TIMEOUT_SECONDS;
    return { timeout: input.timeout, changed: true };
  }
  if (typeof provided !== 'number' || !Number.isFinite(provided) || provided <= 0) {
    throw new BashTimeoutPolicyError(`invalid bash timeout ${JSON.stringify(provided)}: expected a finite positive number of seconds`);
  }
  if (provided > MAX_TIMEOUT_SECONDS) {
    input.timeout = MAX_TIMEOUT_SECONDS;
    return { timeout: input.timeout, changed: true };
  }
  return { timeout: provided, changed: false };
}

export default function bashTimeoutExtension(pi) {
  pi.on('tool_call', async (event) => {
    if (event?.toolName !== 'bash' || !event.input) return undefined;
    try {
      applyBashTimeoutPolicy(event.input);
      return undefined;
    } catch (error) {
      if (!(error instanceof BashTimeoutPolicyError)) throw error;
      return { block: true, reason: `githux dispatcher bash timeout policy: ${error.message}` };
    }
  });
}
