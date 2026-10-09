import { MAX_NAME_LENGTH, parseName } from "@{{name}}/shared";

/** A hint under the name field, or `""` when the name is fine. */
export function nameHint(input: string): string {
  if (parseName(input) !== null) {
    return "";
  }
  return input.trim() === "" ? "Enter a name." : `Use at most ${MAX_NAME_LENGTH} characters.`;
}
