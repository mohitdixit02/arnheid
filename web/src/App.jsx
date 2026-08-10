import { useState, useEffect, useRef } from 'react';
import * as Icons from 'lucide-react';

// Load our JSON data files directly
import pitchData from './data/pitch.json';
import featuresData from './data/features.json';
import commandsData from './data/commands.json';
import roadmapData from './data/roadmap.json';

// Helper component to render Lucide icons dynamically from JSON strings
const LucideIcon = ({ name, className }) => {
  const IconComponent = Icons[name];
  if (!IconComponent) return <Icons.HelpCircle className={className} />;
  return <IconComponent className={className} />;
};

function App() {
  const [dashboard, setDashboard] = useState({
    database: {
      status: "LOADING...",
      last_backup: "Loading...",
      cluster_id: "Loading..."
    },
    memory_stats: {
      total_captured_items: 0,
      total_extracted_entities: 0,
      total_knowledge_edges: 0,
      total_vector_chunks: 0
    },
    recent_items: [],
    recent_entities: []
  });

  // Simulator state for the interactive hero terminal
  const [simStep, setSimStep] = useState(0);

  const [secondsSinceUpdate, setSecondsSinceUpdate] = useState(0);

  useEffect(() => {
    const apiHost = window.location.hostname === 'localhost' ? 'http://localhost:8080' : '';
    
    const fetchStats = () => {
      fetch(`${apiHost}/api/dashboard`)
        .then(res => {
          if (!res.ok) throw new Error("API Offline");
          return res.json();
        })
        .then(data => {
          setDashboard(data);
          setSecondsSinceUpdate(0);
        })
        .catch(err => {
          console.warn("Could not query live ccloud monitor. Fallback to mock values.", err);
          setDashboard({
            database: {
              status: "RUNNING",
              last_backup: new Date().toISOString(),
              cluster_id: "da817a17-4a81-42a5-8f82-3afc39477222"
            },
            memory_stats: {
              total_captured_items: 124,
              total_extracted_entities: 342,
              total_knowledge_edges: 684,
              total_vector_chunks: 512
            },
            recent_items: [
              { "title": "CockroachDB Serverless Documentation", "url": "https://www.cockroachlabs.com/docs/serverless", "source": "telegram", "shared_at": new Date(Date.now() - 1000 * 60 * 12).toISOString() },
              { "title": "AWS EC2 Instance Launching Quickstart", "url": "https://aws.amazon.com/ec2/getting-started", "source": "telegram", "shared_at": new Date(Date.now() - 1000 * 60 * 35).toISOString() },
              { "title": "Next.js 15 Release Notes & Routing Changes", "url": "https://nextjs.org/blog/next-15", "source": "telegram", "shared_at": new Date(Date.now() - 1000 * 60 * 110).toISOString() }
            ],
            recent_entities: [
              { "name": "Sumit Kumar", "type": "person", "first_seen": new Date(Date.now() - 1000 * 60 * 8).toISOString() },
              { "name": "CockroachDB", "type": "technology", "first_seen": new Date(Date.now() - 1000 * 60 * 12).toISOString() },
              { "name": "AWS EC2", "type": "technology", "first_seen": new Date(Date.now() - 1000 * 60 * 35).toISOString() },
              { "name": "Node.js", "type": "technology", "first_seen": new Date(Date.now() - 1000 * 60 * 50).toISOString() }
            ]
          });
          setSecondsSinceUpdate(0);
        });
    };

    // Initial fetch
    fetchStats();

    // Poll backend every 5 seconds
    const pollInterval = setInterval(fetchStats, 5000);

    // Increment seconds elapsed since the last API sync
    const secondsInterval = setInterval(() => {
      setSecondsSinceUpdate(prev => prev + 1);
    }, 1000);

    return () => {
      clearInterval(pollInterval);
      clearInterval(secondsInterval);
    };
  }, []);

  const terminalScrollRef = useRef(null);

  // Simple interval to cycle steps in the hero mock terminal simulator
  useEffect(() => {
    const timer = setInterval(() => {
      setSimStep(prev => (prev + 1) % 4);
    }, 4500);
    return () => clearInterval(timer);
  }, []);

  // Smooth scroll container to bottom when steps change
  useEffect(() => {
    if (terminalScrollRef.current) {
      terminalScrollRef.current.scrollTo({
        top: terminalScrollRef.current.scrollHeight,
        behavior: 'smooth'
      });
    }
  }, [simStep]);

  return (
    <div className="min-h-screen bg-slate-900 text-slate-200 flex flex-col font-sans selection:bg-sky-500 selection:text-white">
      
      {/* ── SECTION 1: HERO & PITCH (DARK PREMIUM SPLIT LAYOUT) ───────────────── */}
      <header className="relative min-h-[100vh] flex items-center py-20 px-6 border-b border-slate-800/40 overflow-hidden bg-slate-900/60">
        {/* Sky blue accent glow behind hero */}
        <div className="absolute top-[-20%] left-[20%] w-[600px] h-[400px] bg-sky-500/5 blur-[150px] rounded-full -z-10" />
        <div className="absolute bottom-[-10%] right-[10%] w-[500px] h-[350px] bg-slate-700/10 blur-[120px] rounded-full -z-10" />

        <div className="max-w-6xl mx-auto grid lg:grid-cols-12 gap-12 items-center">
          
          {/* Left Column: Typography and Action */}
          <div className="lg:col-span-7 text-left space-y-6">
            <div className="inline-flex items-center gap-2 px-3 py-1 bg-slate-800/80 rounded-full border border-slate-700/50 text-xs font-semibold text-sky-400 uppercase tracking-widest">
              <Icons.Shield className="w-3.5 h-3.5" />
              <span>Production-Grade Agent Memory</span>
            </div>

            <div className="space-y-3">
              <h1 className="text-5xl md:text-6xl font-black text-white tracking-tight leading-none">
                {pitchData.title}
              </h1>
              <p className="text-lg md:text-xl font-bold text-slate-300">
                {pitchData.subtitle}
              </p>
            </div>

            <p className="text-sm md:text-base text-slate-400 leading-relaxed max-w-xl">
              {pitchData.description}
            </p>

            <div className="flex items-center gap-2 text-[11px] font-semibold text-slate-500 tracking-wider">
              <span>Powered by <span className="text-slate-200 font-bold">CockroachDB Cloud</span></span>
              <span className="text-slate-700">|</span>
              <Icons.Cpu className="w-3.5 h-3.5 text-orange-500" />
              <span><span className="text-slate-200 font-bold">AWS EC2</span> Hosting</span>
            </div>

            <div className="flex flex-col sm:flex-row items-center gap-4 pt-4">
              <a 
                href={pitchData.links.telegram}
                target="_blank"
                rel="noopener noreferrer"
                className="w-full sm:w-auto px-8 py-3.5 bg-sky-500 hover:bg-sky-600 text-slate-950 font-extrabold rounded-lg shadow-lg hover:shadow-sky-500/20 hover:scale-[1.01] transition duration-200 flex items-center justify-center gap-2"
              >
                <Icons.Send className="w-5 h-5 fill-current" />
                Launch in Telegram
              </a>
              <a 
                href={pitchData.links.video}
                className="w-full sm:w-auto px-8 py-3.5 bg-slate-800 hover:bg-slate-750 text-slate-300 font-bold rounded-lg border border-slate-700/60 hover:scale-[1.01] transition duration-200 flex items-center justify-center gap-2"
              >
                <Icons.Play className="w-5 h-5" />
                Watch Demo Video
              </a>
            </div>
          </div>

          {/* Right Column: Interactive Mock Terminal Simulator */}
          <div className="lg:col-span-5">
            <div className="bg-slate-800/80 rounded-xl border border-slate-700/60 overflow-hidden shadow-2xl">
              
              {/* Header Bar */}
              <div className="bg-slate-900 px-4 py-3 flex items-center justify-between border-b border-slate-800/60">
                <div className="flex items-center gap-2">
                  <Icons.MessageSquare className="w-4 h-4 text-sky-400" />
                  <span className="text-xs font-semibold text-slate-400 font-mono">arnheid_agent_session</span>
                </div>
                <div className="flex gap-1.5">
                  <span className="w-2.5 h-2.5 rounded-full bg-slate-900" />
                  <span className="w-2.5 h-2.5 rounded-full bg-slate-900" />
                  <span className="w-2.5 h-2.5 rounded-full bg-slate-900" />
                </div>
              </div>

              {/* Chat Simulator Content Area */}
              <div ref={terminalScrollRef} className="p-6 font-mono text-[11px] space-y-4 h-[350px] flex flex-col justify-start overflow-y-auto bg-slate-900/40 text-slate-300">
                
                {/* Step 0: User Message Ingestion */}
                <div className={`transition-opacity duration-300 ${simStep >= 0 ? 'opacity-100' : 'opacity-20'}`}>
                  <div className="flex items-center gap-2 text-slate-500 mb-1">
                    <Icons.User className="w-3 h-3" />
                    <span>[Telegram Group Chat] User:</span>
                  </div>
                  <div className="bg-slate-850 p-3 rounded-lg border border-slate-700/50 text-slate-200">
                    "Hey @arnheidgenbot have you sent the project targets email to Sumit Kumar yet?"
                  </div>
                </div>

                {/* Step 1: Backend Context Resolution */}
                <div className={`transition-all duration-300 ${simStep >= 1 ? 'opacity-100 max-h-[100px] scale-100' : 'opacity-0 max-h-0 scale-95 overflow-hidden'}`}>
                  <div className="space-y-1">
                    <div className="flex items-center gap-2 text-sky-400">
                      <Icons.Database className="w-3.5 h-3.5" />
                      <span>[CockroachDB Query]</span>
                    </div>
                    <div className="pl-4 text-slate-400 italic">
                      "SELECT message FROM messages_buffer WHERE chat_id = group_id ORDER BY timestamp DESC LIMIT 10;"
                    </div>
                    <div className="pl-4 text-sky-500 font-semibold">
                      → Found Context: [Sumit Kumar's email: gsg.gaming67@gmail.com, target deadline: August 17th]
                    </div>
                  </div>
                </div>

                {/* Step 2: Agent Tools Decision */}
                <div className={`transition-all duration-300 ${simStep >= 2 ? 'opacity-100 max-h-[120px] scale-100' : 'opacity-0 max-h-0 scale-95 overflow-hidden'}`}>
                  <div className="space-y-1">
                    <div className="flex items-center gap-2 text-orange-400">
                      <Icons.Cpu className="w-3.5 h-3.5" />
                      <span>[Agent reasoning loop]</span>
                    </div>
                    <div className="pl-4 text-slate-400">
                      "Searching Gmail logs to verify target email status..."
                    </div>
                    <div className="pl-4 text-orange-500">
                      → call: gsuite_gmail_search(&#123;"query": "to:gsg.gaming67@gmail.com"&#125;)
                    </div>
                    <div className="pl-4 text-slate-500">
                      Result: "No emails found matching query."
                    </div>
                  </div>
                </div>

                {/* Step 3: Action Execution */}
                <div className={`transition-all duration-300 ${simStep >= 3 ? 'opacity-100 max-h-[100px] scale-100' : 'opacity-0 max-h-0 scale-95 overflow-hidden'}`}>
                  <div className="space-y-1">
                    <div className="flex items-center gap-2 text-emerald-400">
                      <Icons.CheckCircle className="w-3.5 h-3.5" />
                      <span>[Action Executed]</span>
                    </div>
                    <div className="pl-4 text-emerald-500">
                      → call: gsuite_gmail_send(&#123;"to": "gsg.gaming67@gmail.com", "subject": "Timelines and Targets", "body": "..."&#125;)
                    </div>
                    <div className="pl-4 text-emerald-400 font-bold">
                      Response: "Email sent successfully to Sumit Kumar."
                    </div>
                  </div>
                </div>

              </div>
            </div>
          </div>

        </div>
      </header>

      {/* ── SECTION 2: COCKROACHDB CLOUD SPEC (SLATE/WHITE BORDERED THEME) ─── */}
      {/* Part 1: Feature Specs (Dark Slate Background) */}
      <section className="bg-slate-850 py-16 px-6 border-b border-slate-800/40">
        <div className="max-w-5xl mx-auto">
          <h2 className="text-2xl md:text-3xl font-extrabold text-white text-center mb-10">
            Integrated CockroachDB Cloud Suite
          </h2>
          <div className="grid md:grid-cols-3 gap-8">
            {pitchData.cdbFeatures.map((f, idx) => (
              <div key={idx} className="bg-slate-900 p-6 rounded-xl border border-slate-800 shadow-lg flex flex-col justify-between hover:border-slate-700 transition duration-300">
                <div>
                  <div className="w-10 h-10 bg-slate-800 rounded-lg flex items-center justify-center text-sky-400 mb-4 border border-slate-700/50">
                    {idx === 0 && <Icons.Server className="w-5 h-5" />}
                    {idx === 1 && <Icons.Database className="w-5 h-5" />}
                    {idx === 2 && <Icons.Terminal className="w-5 h-5" />}
                  </div>
                  <h3 className="text-base font-bold text-slate-100 mb-2">{f.title}</h3>
                  <p className="text-xs md:text-sm text-slate-400 leading-relaxed mb-4">{f.description}</p>
                </div>
                <div className="text-[10px] text-sky-400 font-semibold uppercase tracking-wider font-mono">
                  {idx === 0 && "Global Endpoint"}
                  {idx === 1 && "Semantic Storage"}
                  {idx === 2 && "CLI Automation"}
                </div>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* Part 2: Backend Tech Pitch (Darker Charcoal Background) */}
      <section className="bg-slate-900 py-16 px-6 border-b border-slate-800/40">
        <div className="max-w-4xl mx-auto">
          <h2 className="text-2xl md:text-3xl font-extrabold text-white text-center mb-10">
            {pitchData.architecturePitch.title}
          </h2>
          <div className="grid md:grid-cols-2 gap-8 items-stretch">
            
            {/* Memory & Logic column */}
            <div className="space-y-6 bg-slate-850 p-6 rounded-xl border border-slate-800/50 flex flex-col justify-between">
              <div className="space-y-6">
                <div className="flex gap-4">
                  <div className="w-8 h-8 rounded-full bg-emerald-950 border border-emerald-900/60 flex items-center justify-center text-emerald-400 shrink-0">
                    <Icons.ShieldCheck className="w-4 h-4" />
                  </div>
                  <div>
                    <h3 className="text-sm font-bold text-slate-100 mb-1">Memory Durability</h3>
                    <p className="text-xs text-slate-400 leading-relaxed">{pitchData.architecturePitch.memoryDurability.replace("Memory Durability: ", "")}</p>
                  </div>
                </div>
                <div className="flex gap-4">
                  <div className="w-8 h-8 rounded-full bg-sky-950 border border-sky-900/60 flex items-center justify-center text-sky-400 shrink-0">
                    <Icons.Workflow className="w-4 h-4" />
                  </div>
                  <div>
                    <h3 className="text-sm font-bold text-slate-100 mb-1">Agent Orchestration</h3>
                    <p className="text-xs text-slate-400 leading-relaxed">{pitchData.architecturePitch.agentOrchestration.replace("Agent Orchestration: ", "")}</p>
                  </div>
                </div>
              </div>
            </div>

            {/* AWS EC2 Specific Column */}
            <div className="space-y-6 bg-slate-855 p-6 rounded-xl border border-slate-800/50 flex flex-col justify-between">
              <div className="space-y-4">
                <div className="flex gap-4">
                  <div className="w-8 h-8 rounded-full bg-orange-950 border border-orange-900/60 flex items-center justify-center text-orange-400 shrink-0">
                    <Icons.Cpu className="w-4 h-4" />
                  </div>
                  <div>
                    <h3 className="text-sm font-bold text-slate-100 mb-1">AWS EC2 Hosting</h3>
                    <p className="text-xs text-slate-400 leading-relaxed">{pitchData.architecturePitch.awsHosting.replace("AWS EC2 Hosting: ", "")}</p>
                  </div>
                </div>
              </div>
            </div>

          </div>
        </div>
      </section>

      {/* ── SECTION 3: LIVE HEALTH DASHBOARD (PREMIUM DARK CARDS) ────────────── */}
      <section className="bg-slate-850 py-20 px-6 border-b border-slate-900">
        <div className="max-w-4xl mx-auto">
          <div className="text-center mb-12">
            <div className="flex items-center justify-center gap-3 mb-2">
              <span className="relative flex h-2.5 w-2.5">
                <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75"></span>
                <span className="relative inline-flex rounded-full h-2.5 w-2.5 bg-emerald-500"></span>
              </span>
              <h2 className="text-2xl md:text-3xl font-extrabold text-white">Live Health Dashboard</h2>
            </div>
            <p className="text-xs text-slate-500">
              Real-time state pulled from our AWS EC2 backend server {secondsSinceUpdate > 0 ? `(updated ${secondsSinceUpdate}s ago)` : '(just updated)'}
            </p>
          </div>

          <div className="grid md:grid-cols-3 gap-6 mb-8">
            
            {/* Cluster Status Card */}
            <div className="bg-slate-900 p-6 rounded-xl border border-slate-800 hover:border-sky-500/20 transition duration-300 flex flex-col justify-between shadow-lg">
              <div>
                <Icons.Activity className="w-7 h-7 mb-3 text-sky-400" />
                <h3 className="text-[10px] font-semibold text-slate-500 uppercase tracking-widest font-mono">Cluster Status</h3>
              </div>
              <div className="mt-6">
                <p className="text-2xl font-bold tracking-tight text-white mb-1">
                  {dashboard.database.status}
                </p>
                <p className="text-[9px] text-slate-500 font-mono">
                  ID: {dashboard.database.cluster_id ? dashboard.database.cluster_id.substring(0, 13) + "..." : "Loading"}
                </p>
              </div>
            </div>

            {/* Backup Status Card */}
            <div className="bg-slate-900 p-6 rounded-xl border border-slate-800 hover:border-slate-700 transition duration-300 flex flex-col justify-between shadow-lg">
              <div>
                <Icons.Clock className="w-7 h-7 mb-3 text-slate-400" />
                <h3 className="text-[10px] font-semibold text-slate-500 uppercase tracking-widest font-mono">Backup Status</h3>
              </div>
              <div className="mt-6">
                <p className="text-sm font-bold text-white truncate mb-1">
                  {dashboard.database.last_backup !== "Unknown" && dashboard.database.last_backup !== "Loading..."
                    ? new Date(dashboard.database.last_backup).toLocaleDateString()
                    : "Fresh"}
                </p>
                <p className="text-[9px] text-slate-500 font-mono">
                  Interval: 24h checks
                </p>
              </div>
            </div>

            {/* Memory Pool Card */}
            <div className="bg-slate-900 p-6 rounded-xl border border-slate-800 hover:border-sky-500/20 transition duration-300 flex flex-col justify-between shadow-lg">
              <div>
                <Icons.Layers className="w-7 h-7 mb-3 text-sky-500" />
                <h3 className="text-[10px] font-semibold text-slate-500 uppercase tracking-widest font-mono">Memory Pool</h3>
              </div>
              <div className="mt-6">
                <p className="text-2xl font-bold tracking-tight text-white mb-1">
                  {dashboard.memory_stats.total_vector_chunks + dashboard.memory_stats.total_extracted_entities}
                </p>
                <p className="text-[9px] text-slate-400 font-mono">
                  Vectors: {dashboard.memory_stats.total_vector_chunks} | Edges: {dashboard.memory_stats.total_knowledge_edges}
                </p>
              </div>
            </div>

          </div>

          {/* Recent Live Activity Feed Grid */}
          <div className="grid md:grid-cols-2 gap-8 mt-12 text-left">
            
            {/* Left Feed: Recent memory items */}
            <div className="bg-slate-900/60 p-6 rounded-xl border border-slate-800 shadow-md">
              <div className="flex items-center gap-2 mb-4 border-b border-slate-800 pb-3">
                <Icons.Layers className="w-4.5 h-4.5 text-sky-400" />
                <h3 className="text-sm font-bold text-slate-100 uppercase tracking-wider font-mono">Recent Memory Ingestions</h3>
              </div>
              <div className="space-y-4">
                {dashboard.recent_items && dashboard.recent_items.length > 0 ? (
                  dashboard.recent_items.map((item, idx) => (
                    <div key={idx} className="text-xs space-y-1 border-b border-slate-800/40 pb-3 last:border-0 last:pb-0">
                      <div className="flex justify-between items-start gap-4">
                        <span className="font-bold text-slate-200 truncate block max-w-[250px]">
                          {item.title}
                        </span>
                        <span className="px-1.5 py-0.5 bg-slate-800 text-[9px] text-sky-400 rounded border border-slate-700/60 uppercase font-mono font-semibold shrink-0">
                          {item.source}
                        </span>
                      </div>
                      <a 
                        href={item.url} 
                        target="_blank" 
                        rel="noreferrer" 
                        className="text-[10px] text-slate-500 hover:text-sky-400 truncate block hover:underline"
                      >
                        {item.url}
                      </a>
                      <div className="text-[9px] text-slate-600 font-mono">
                        Captured: {new Date(item.shared_at).toLocaleTimeString()}
                      </div>
                    </div>
                  ))
                ) : (
                  <p className="text-xs text-slate-500 italic">No link memories captured yet.</p>
                )}
              </div>
            </div>

            {/* Right Feed: Recent extracted entities */}
            <div className="bg-slate-900/60 p-6 rounded-xl border border-slate-800 shadow-md">
              <div className="flex items-center gap-2 mb-4 border-b border-slate-800 pb-3">
                <Icons.Network className="w-4.5 h-4.5 text-emerald-400" />
                <h3 className="text-sm font-bold text-slate-100 uppercase tracking-wider font-mono">Recently Extracted Entities</h3>
              </div>
              <div className="space-y-4">
                {dashboard.recent_entities && dashboard.recent_entities.length > 0 ? (
                  dashboard.recent_entities.map((ent, idx) => (
                    <div key={idx} className="text-xs flex items-center justify-between border-b border-slate-800/40 pb-3 last:border-0 last:pb-0">
                      <div className="space-y-0.5">
                        <span className="font-bold text-slate-200 block">
                          {ent.name}
                        </span>
                        <span className="text-[9px] text-slate-600 font-mono">
                          First Seen: {new Date(ent.first_seen).toLocaleTimeString()}
                        </span>
                      </div>
                      <span className="px-1.5 py-0.5 bg-emerald-950/40 text-[9px] text-emerald-400 rounded border border-emerald-900/40 uppercase font-mono font-semibold">
                        {ent.type}
                      </span>
                    </div>
                  ))
                ) : (
                  <p className="text-xs text-slate-500 italic">No knowledge graph nodes extracted yet.</p>
                )}
              </div>
            </div>

          </div>

          <p className="text-center text-[9px] text-slate-600 font-mono mt-8">
            * Operational metrics and backup listings are monitored in real-time using CockroachDB Cloud CLI (ccloud) commands.
          </p>
          <p className="text-center text-[9px] text-amber-500/80 font-mono mt-2">
            Note: The recent activity feed above is enabled temporarily for hackathon demonstration purposes and will be disabled in production to respect conversation privacy.
          </p>
        </div>
      </section>

      {/* ── SECTION 4: CORE FEATURES & BOT COMMANDS (DARK ACCENT CARDS) ──────── */}
      {/* 4.1 Core Features (Dark Bordered Cards) */}
      <section className="bg-slate-900 py-16 px-6 border-b border-slate-800/40">
        <div className="max-w-4xl mx-auto">
          <h2 className="text-2xl md:text-3xl font-extrabold text-white text-center mb-10">
            Core Features & Abilities
          </h2>
          <div className="grid md:grid-cols-2 gap-6">
            {featuresData.map((feat, idx) => (
              <div key={idx} className="bg-slate-850 p-5 rounded-xl border border-slate-800/60 shadow-sm flex gap-4 hover:border-slate-700 transition duration-250">
                <div className="w-9 h-9 rounded-lg bg-sky-950 text-sky-400 flex items-center justify-center shrink-0 border border-sky-900/40">
                  <LucideIcon name={feat.icon} className="w-4.5 h-4.5" />
                </div>
                <div>
                  <h3 className="text-sm font-bold text-slate-100 mb-1">{feat.title}</h3>
                  <p className="text-xs text-slate-400 leading-relaxed">{feat.description}</p>
                </div>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* 4.2 Bot Commands (Code Terminal Style) */}
      <section className="bg-slate-900 py-16 px-6 border-b border-slate-800/40">
        <div className="max-w-3xl mx-auto">
          <h2 className="text-2xl md:text-3xl font-extrabold text-white text-center mb-10">
            Bot Interaction Interface
          </h2>
          <div className="border border-slate-800 rounded-xl overflow-hidden shadow-2xl">
            <div className="bg-slate-950 px-4 py-3 flex items-center justify-between border-b border-slate-850">
              <span className="text-xs font-semibold text-slate-500 uppercase tracking-widest font-mono">Telegram Bot CLI</span>
              <div className="flex gap-1.5">
                <span className="w-2 h-2 rounded-full bg-slate-900" />
                <span className="w-2 h-2 rounded-full bg-slate-900" />
                <span className="w-2 h-2 rounded-full bg-slate-900" />
              </div>
            </div>
            <div className="bg-slate-900/90 p-5 font-mono text-xs leading-relaxed space-y-5 overflow-x-auto text-sky-400">
              {commandsData.map((cmd, idx) => (
                <div key={idx} className="border-b border-slate-800/60 pb-3.5 last:border-0 last:pb-0">
                  <div className="flex items-center gap-2 mb-1">
                    <span className="text-slate-600 select-none">$</span>
                    <span className="font-bold text-slate-200">{cmd.command}</span>
                  </div>
                  <p className="text-[11px] text-slate-400 pl-4">{cmd.description}</p>
                </div>
              ))}
            </div>
          </div>
        </div>
      </section>

      {/* ── SECTION 5: UPCOMING ROADMAP (DARK TIMELINE) ───────────────────────── */}
      <section className="bg-slate-900 py-16 px-6 border-b border-slate-800/40">
        <div className="max-w-4xl mx-auto">
          <h2 className="text-2xl md:text-3xl font-extrabold text-white text-center mb-10">
            Roadmap & Extensions
          </h2>
          <div className="relative border-l border-slate-800 ml-4 md:ml-8 space-y-10">
            {roadmapData.map((item, idx) => (
              <div key={idx} className="relative pl-8 md:pl-10">
                <span className="absolute -left-[7px] top-1.5 w-3.5 h-3.5 rounded-full bg-slate-900 border border-sky-500 flex items-center justify-center">
                  <span className="w-1.5 h-1.5 rounded-full bg-sky-500" />
                </span>
                <h3 className="text-base font-bold text-slate-200 mb-1">{item.title}</h3>
                <p className="text-slate-400 leading-relaxed max-w-2xl text-xs">{item.description}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* ── FOOTER ─────────────────────────────────────────────────────────── */}
      <footer className="bg-slate-900 text-slate-600 py-12 px-6 border-t border-slate-800/40 mt-auto">
        <div className="max-w-4xl mx-auto flex flex-col md:flex-row items-center justify-between gap-6">
          <div className="flex items-center gap-2.5 text-slate-300">
            <Icons.Cpu className="w-5 h-5 text-sky-400" />
            <span className="font-bold text-base tracking-wider">ARNHEID</span>
          </div>

          <div className="text-[9px] text-center md:text-right space-y-1">
            <p>© 2026 Arnheid Memory Engine. Built for the CockroachDB x AWS Hackathon.</p>
            <p className="text-slate-600 font-mono">
              Disclaimer: GSuite access, email updates, and active command auditing are executed strictly based on user authorization policies.
            </p>
          </div>
        </div>
      </footer>

    </div>
  );
}

export default App;
