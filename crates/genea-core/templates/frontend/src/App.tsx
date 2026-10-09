import { useState } from "react";
import { clickLabel } from "./clicks";
import "./App.css";

export function App() {
  const [clicks, setClicks] = useState(0);

  return (
    <main>
      <h1>{{name}}</h1>
      <button type="button" onClick={() => setClicks((count) => count + 1)}>
        {clickLabel(clicks)}
      </button>
      <p>
        Edit <code>src/App.tsx</code> and save to see it change.
      </p>
    </main>
  );
}
