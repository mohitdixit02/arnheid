# Plan: React Landing Page & Live Dashboard

This plan outlines the architecture, design pages, and implementation steps to build a single-page React application serving as both a startup pitch for Arnheid and an operational status dashboard.

---

## 1. Web Application Pages & Structure

We will design a single-page React application (using TailwindCSS for layout, and Lucide icons for visual assets).

### Section 1: The Pitch (Above the Fold)
* **Heading**: "Arnheid: The Autonomous Memory Engine for Team Communication"
* **Subheading**: "An AI agent that sits in your Telegram & WhatsApp chats, constructs structured knowledge graphs, and automates real-world tasks natively powered by CockroachDB Cloud."
* **Primary Actions**:
  * **"Launch in Telegram"** (Link to `t.me/arnheidgenbot`)
  * **"Watch Demo Video"** (Scrolls to video embed section)

### Section 2: Technical Architecture & CockroachDB
* **Visual Stack Diagram**: Render the pipeline (Ingress → Rust Parsing → Vector Indexing in CockroachDB → MCP Action Orchestration).
* **Focus Cards**:
  * **Memory Durability**: How CockroachDB hosts user profiles, sessions, and vector databases natively.
  * **Agent Orchestration**: Details on GSuite MCP tools (Gmail, Calendar, Drive).
  * **AWS Infrastructure**: Details on EC2 containerized hosting.

### Section 3: Live Health Dashboard (Operational Metrics)
A real-time metrics console that queries the Rust backend API:
* **Cluster Status Card**: Displays the current CockroachDB Cloud cluster state (e.g., `RUNNING` / Healthy) fetched from the backend's `ccloud` CLI monitor logs.
* **Backup Freshness Indicator**: Shows the time of the latest database backup.
* **Memory Pool Stats**: Displays the number of entities, relationships, and vector chunks stored in the database.

---

## 2. API Endpoints Needed (Rust Backend)

To power the dashboard, we will expose a simple read-only API endpoint from the Rust application server (`src/http.rs`):

* **`GET /api/dashboard`**:
  * **Response Schema**:
    ```json
    {
      "database": {
        "status": "RUNNING",
        "last_backup": "2026-08-16T12:00:00Z",
        "cluster_id": "da817a17-4a81-42a5-8f82-3afc39477222"
      },
      "memory_stats": {
        "total_captured_items": 142,
        "total_extracted_entities": 412,
        "total_knowledge_edges": 843
      }
    }
    ```

---

## 3. Next Steps

1. **User Design Review**: You review this structure and let me know if you have specific layout designs or assets you want to include.
2. **Backend API Prep**: We will write the `GET /api/dashboard` handler in the Rust codebase.
3. **React Initialization**: We will initialize the React project directory using Vite (`npx create-vite`), set up Tailwind, and write the application files.
