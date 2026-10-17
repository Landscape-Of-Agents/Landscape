# Install Crane

Landscape is the editor side of **Crane**. Every action runs the Crane CLI, so Crane must be installed first.

1. Download Crane **0.4.10 or newer** from the releases page.
2. Put it on your `PATH`, or set **Landscape: Crane Path** (`foxx.cranePath`) to the executable.
3. Check it with `crane --version`.

Landscape checks the version when it starts. An older Crane cannot sign governance changes, so Landscape says so in the status bar and in the overview instead of running commands that would fail.
