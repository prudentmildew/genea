/** The longest name the API accepts. */
export const MAX_NAME_LENGTH = 40;

/**
 * Checks a name the user typed. Returns it trimmed, or `null` if it is empty
 * or too long. The web app and the API both use it, so they always agree.
 */
export function parseName(input: string): string | null {
  const name = input.trim();
  return name.length > 0 && name.length <= MAX_NAME_LENGTH ? name : null;
}
