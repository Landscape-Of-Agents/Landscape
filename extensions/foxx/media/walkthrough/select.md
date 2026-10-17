# Preserve or target code

Select lines in the editor and press **Ctrl+Alt+F** (or use the Landscape button in the editor title).

| Action | What it means |
|---|---|
| **Preserve** | Agents may not change these lines. Crane denies the tool call and records it. |
| **Target** | These are the lines an agent is expected to change. |
| **Zone** | A risky area with an autonomy ceiling and context the agent receives. |
| **Flow** | A path through the code that starts at an entry point. |

Preserved and targeted lines are highlighted. A code lens above them shows the policy and the last test outcome. Hover over them for details and links.
