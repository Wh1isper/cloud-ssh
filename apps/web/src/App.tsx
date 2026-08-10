const foundations = [
  {
    title: "Target-owned sessions",
    detail: "tmux on your machine owns every pane, process, and scrollback buffer.",
  },
  {
    title: "Outbound Relay",
    detail: "A small target-side client connects outward and carries SSH back to OwlMux Server.",
  },
  {
    title: "Web roaming",
    detail: "Reconnect from a browser and rebuild the graphical workspace from live tmux state.",
  },
];

export function App() {
  return (
    <main className="shell">
      <nav className="nav" aria-label="Primary navigation">
        <a className="brand" href="/" aria-label="OwlMux home">
          <span className="brand-mark" aria-hidden="true">
            OM
          </span>
          <span>OwlMux</span>
        </a>
        <div className="nav-links">
          <a href="https://owlmux-docs.owlfoundry.org">Docs</a>
          <a href="https://github.com/owlfoundry/owlmux">GitHub</a>
        </div>
      </nav>

      <section className="hero">
        <div className="status-pill">
          <span className="status-dot" aria-hidden="true" />
          Foundation
        </div>
        <p className="eyebrow">Terminal roaming, without moving the session</p>
        <h1>Your tmux sessions stay where they belong.</h1>
        <p className="lede">
          OwlMux is a self-hosted Web client and reverse connection path for target-owned tmux. The
          browser can leave. The Server can restart. Your process remains on your machine.
        </p>
        <div className="actions">
          <a
            className="primary-action"
            href="https://owlmux-docs.owlfoundry.org/guide/architecture"
          >
            Read the architecture
          </a>
          <a
            className="secondary-action"
            href="https://github.com/owlfoundry/owlmux/tree/main/spec"
          >
            Review the specifications
          </a>
        </div>
      </section>

      <section className="foundations" aria-labelledby="foundation-heading">
        <div className="section-heading">
          <p className="eyebrow">The boundary</p>
          <h2 id="foundation-heading">One durable owner. Replaceable paths.</h2>
        </div>
        <div className="card-grid">
          {foundations.map((foundation, index) => (
            <article className="card" key={foundation.title}>
              <span className="card-number">0{index + 1}</span>
              <h3>{foundation.title}</h3>
              <p>{foundation.detail}</p>
            </article>
          ))}
        </div>
      </section>

      <section className="notice" aria-labelledby="status-heading">
        <div>
          <p className="eyebrow">Current status</p>
          <h2 id="status-heading">A clean product foundation</h2>
        </div>
        <p>
          This build intentionally contains no login, machine registration, Relay tunnel, SSH, or
          tmux integration yet. Those capabilities land only after their end-to-end acceptance gates
          pass.
        </p>
      </section>
    </main>
  );
}
