This is the layout_Schema for proper design of web page while structure still will be followed based on `react_app_plan.md` file.

Overall color scheme: Light blue, white, and gray with accent colors for buttons and highlights - refer cockroachdb cloud branding for inspiration.

### Section1:
Arnheid - Bold Capital font light blue scheme with shadow effect.
Powered by CockroachDB Cloud - smaller font, gray color, italicized. x AWS (mention AWS logo here) - smaller font, gray color, italicized.
Subheading: "Include Telegram only for now" - color gray, regular font.
Primary Action Buttons:
- "Launch in Telegram" - Light blue button with white text, rounded corners, hover effect
- "Watch Demo Video" - Gray button with white text, rounded corners, hover effect

### Section2:
Visual Stack Diagram: Use a clean, modern illustration style with light blue and gray tones. Include icons for each component (Ingress, Rust Parsing, Vector Indexing, MCP Action Orchestration).

Divide it in two sectinos:
1. Content should mention the three things that we implement using cockroachdb cloud for our backend and how it helps with memory durability, agent orchestration, and AWS infrastructure - vector indexing, mcp, and cli.
```from hackathon doc
- CockroachDB Cloud Managed MCP Server — Connect AI agents directly to CockroachDB clusters with a single config snippet from the Cloud Console. Works natively with Claude Code, Cursor, and VS Code. Safe by default: read-only mode, full audit logging, zero custom proxy required. Endpoint: https://cockroachlabs.cloud/mcp
- CockroachDB Distributed Vector Indexing — Store and query embeddings at scale using CockroachDB's vector support with distributed indexing. Semantic search and retrieval stay fast as your data grows — no separate vector store to maintain, no reindexing pain, and no consistency gaps between your vector data and your operational database. Ideal for RAG pipelines, long-term agent memory, and semantic search applications.
- ccloud CLI (Agent-Ready) — Give your agent direct, secure access to the full CockroachDB Cloud control plane. Provision clusters, manage backups, configure networking, monitor audit logs — all from the terminal. Designed for AI with consistent noun-verb patterns, JSON output on every command, and granular service-account-based RBAC
```
2. Content should pitch and showcase why we use cockroachdb cloud for our backend and how it helps with memory durability, agent orchestration, and AWS infrastructure.

Both sections must have different background colors - one light blue and the other white, with clear separation and padding.

Also add a subsection telling about AWS EC2 which we use, as it is a cosponsor for this hackathon, so dont miss it.

### Section3:
Live Health Dashboard (Operational Metrics) - Use a clean, modern card layout with light blue and gray tones. Each card should have a shadow effect and rounded corners.
- Cluster Status Card: Light blue background with white text, displays the current CockroachDB Cloud cluster state (e.g., `RUNNING` / Healthy) fetched from the backend's `ccloud` CLI monitor logs.
- Backup Freshness Indicator: Gray background with white text, shows the time of the latest database backup.
- Memory Pool Stats: Light blue background with white text, displays the number of entities, relationships, and vector chunks stored in the database.

Also mention that it is powered by cockroachdb cloud cli. (make it a small text at the bottom of the dashboard section)


### Section4:
Mention about the corefeatures and functionalities of what Arnheid do. Divide it into 2 sections with indiciual backgrounds, for example, light blue background and white text, rounded corners, and shadow effect.
Section4.1: Arnheid core features and functionalities - ingest text, urls, respond to queries using websearch, smart agentic loop, use gsuite tools, semantic search, build knowledge graph, and more. Use icons for each feature and a brief description below each icon.
Section4.2: Commands and flags availabe in the telegram bot - list the commands and flags available in the telegram bot, with a brief description of each command and flag. Use icons for each command and flag.

Section5:
Mention about the upcoming features and functionalities of what Arnheid will do in the future. Include whatstapp ongoing and other platforms and more mcp tools will come soon.

Last section will be the footer section with links to social media, contact information, and any other relevant links. Use a light gray background with white text and icons for social media links. Include a copyright notice and any necessary disclaimers.

#### Note
while building data, make sure that all the content which appear as text (Like pitch, subheading, features, commands, etc.) should be located in `data\` folder in form of json files and the react app should read from those json files to display the content on the web page. This will make it easier to update the content in the future without changing the code.
