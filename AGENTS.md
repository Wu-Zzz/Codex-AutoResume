# Development rules

- Product scope and fixed decisions are in PLAN.md. Implement them; do not restart generic product planning.
- Keep the UI small: tasks, clear status, automatic continuation switch, short history. Chinese is the initial UI language.
- Design this as a compact desktop utility. No marketing hero, slogans, decorative captions, duplicate action buttons, or auxiliary text below 12 px. Keep each action in one place; put diagnostic explanations behind a clear details entry. Never hide the current observation-only capability boundary.
- Rust owns scheduling and persistence. Vue renders typed views and invokes narrow commands. Never duplicate scheduling in the frontend.
- Use official Codex App Server protocol. Never edit Codex databases, auth files or rollout files. Never collect credentials.
- Fail closed on unknown protocol fields, ambiguous execution ownership, permission/config changes and uncertain turn submission.
- Only explicit terminal usageLimitExceeded permits automatic continuation. Do not classify errors by natural-language matching.
- Persist intent before sending turn/start. An uncertain send must not be retried without reconciliation. Protect against concurrent app instances.
- Approval/user-input requests need the user. Never auto-approve or silently loosen permissions. Preserve all execution configuration or refuse automatic recovery.
- Real account checks are read-only unless the user explicitly authorizes a concrete live execution test. Do not resume or interrupt unrelated user chats during development.
- Add meaningful tests for scheduler transitions, rate-limit gates, persistence/restart and transport failures. Do not add cosmetic tests.
- Validate with npm run check, npm run build, cargo fmt --check, cargo test and cargo clippy where supported. Report unavailable platform checks honestly.
- Keep intermediate tools, generated schemas, logs and screenshots in ignored work/. Never commit personal task lists, email addresses, account details or tokens.
- Prefer straightforward modules and one source of truth. No duplicate services or speculative abstractions.
