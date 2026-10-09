import { useState } from "react";
import { api } from "./api";
import { nameHint } from "./name";
import "./App.css";

export function App() {
  const [name, setName] = useState("");
  const [message, setMessage] = useState("");
  const hint = nameHint(name);

  async function greet() {
    const response = await api.hello.$get({ query: { name } });
    if (response.ok) {
      const body = await response.json();
      setMessage(body.message);
    }
  }

  return (
    <main>
      <h1>{{name}}</h1>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void greet();
        }}
      >
        <input
          value={name}
          onChange={(event) => setName(event.target.value)}
          placeholder="Your name"
        />
        <button type="submit" disabled={hint !== ""}>
          Say hello
        </button>
      </form>
      <p>{name === "" ? "" : hint}</p>
      <p>{message}</p>
    </main>
  );
}
