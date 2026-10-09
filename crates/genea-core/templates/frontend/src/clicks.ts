/** The button's label after `count` clicks. */
export function clickLabel(count: number): string {
  if (count === 0) {
    return "Click me";
  }
  return count === 1 ? "Clicked once" : `Clicked ${count} times`;
}
