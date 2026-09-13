/**
 * The server's password policy, mirrored so the form can say so before a
 * round trip. The server remains the authority: it checks again, and its
 * `422` is shown as is.
 */
export const MIN_PASSWORD_CHARS = 15;
export const MAX_PASSWORD_CHARS = 128;

/** Why a new password would be refused, or `null` if it would not. Counts
 * characters the way the server does -- Unicode scalar values, not UTF-16
 * code units. */
export function passwordProblem(password: string, confirmation: string): string | null {
  const length = [...password].length;
  if (length < MIN_PASSWORD_CHARS) {
    return `Use at least ${MIN_PASSWORD_CHARS} characters.`;
  }
  if (length > MAX_PASSWORD_CHARS) {
    return `Use at most ${MAX_PASSWORD_CHARS} characters.`;
  }
  if (password !== confirmation) {
    return "The two passwords do not match.";
  }
  return null;
}
