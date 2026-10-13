# Set up the repository

**Set Up Crane** connects the clone to its GitHub `origin` remote and runs `crane init`.

- **A new repository:** Crane creates the owner key and records a `baseline` checkpoint. Commit the `.crane` folder.
- **A repository that is already set up:** Foxx shows the owner key's fingerprint. Trust it only if it matches what the owner sees with `crane admin list`.

Until the rules are trusted on this machine, agent changes stay blocked.
