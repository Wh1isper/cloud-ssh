window.addEventListener("DOMContentLoaded", () => {
  const mermaidBlocks = document.querySelectorAll("pre > code.language-mermaid");
  mermaidBlocks.forEach((code) => {
    const pre = code.parentElement;
    const container = document.createElement("div");
    container.className = "mermaid";
    container.textContent = code.textContent;
    pre.replaceWith(container);
  });
});
