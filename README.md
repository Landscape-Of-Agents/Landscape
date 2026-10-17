<p align="center">
  <img src="landing/public/logo.png" alt="Crane" width="96" height="96">
</p>

<h1 align="center">Crane</h1>

<p align="center">
  <b>Guardrails for intelligent agents.</b><br>
  Minimize token usage, incidents, and AI psychosis with richer context, better guardrails, and
  stronger observability for agents.
</p>

<p align="center">
  <a href="https://github.com/VishuKalier2003/Agentscript/releases/latest">Download</a> ·
  <a href="docs/GUIDE.md">Guide</a> ·
  <a href="https://marketplace.visualstudio.com/items?itemName=agentscript.foxx">Foxx for VS Code</a> ·
  <a href="https://agentscript-service.onrender.com">Dashboard</a>
</p>

---

## Why Crane exists

AI agents now read, write, and run code faster than anyone can review it. They are capable, but
they work from whatever context they happen to see, and a prompt is a request, not a rule. The
result is familiar: wasted tokens rediscovering the same constraints, confident changes to code
that should never move, and incidents nobody can explain afterwards.

Crane's vision is that working with an agent should feel like working with a trusted teammate who
knows the house rules: it is told what matters before it acts, it cannot cross the lines that
matter most, and everything it did can be shown afterwards.

## Three ideas

- **Richer context.** Every protected line, zone, and flow carries guidance. The moment an agent
  touches that code, Crane hands it the context it needs, so it stops guessing.
- **Better guardrails.** Rules are written in **AgentScript** and signed. Crane checks every tool
  call before it runs, denies what breaks a rule, and verifies what actually changed on disk.
- **Stronger observability.** Every decision becomes tamper-evident evidence, so you can see which
  agent did what, what was denied, and why, for one developer or a whole team.

## AgentScript in a glance

```text
policy payments {
    preserve K7M2P9RX;                         // the fee rule: agents may not change it
    target   4QZ8M2LD change_type logical_bn;  // the refund: agents must rewrite it
}
```

Mark code with `crane protect` or `crane target`, group risky code into zones and flows, and Crane
enforces the policy in Claude Code, Codex, GitHub Copilot CLI, and Gemini CLI.

## One rulebook, three ways in

| | |
| --- | --- |
| **CLI** | The rules and the referee: signed contracts, tests, and enforcement in the agent's hooks. |
| **Foxx** | The VS Code extension: protect code, draw zones and flows on a graph, and watch the context agents receive. |
| **Dashboard** | Every teammate's agents in one place: decisions, denials, incidents, and tokens. |

## Get involved

- **Use it:** start with the [guide](docs/GUIDE.md).
- **Build it:** read [CONTRIBUTING.md](CONTRIBUTING.md).
- **Report a vulnerability:** see [SECURITY.md](SECURITY.md).

## License

Crane is open source under the [Apache License 2.0](LICENSE.md).
