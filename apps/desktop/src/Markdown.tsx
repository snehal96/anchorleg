import { memo, useEffect, useId, useState, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import remarkGfm from "remark-gfm";
import { openTarget } from "./api";
import { copyText } from "./share";
import { Icon } from "./ui";

/** A link the agent wrote: a web page, or a file in the task's folder, maybe with a line
 * (`src/a.rs:42`, `/abs/a.rs:42:3`, `a.rs:12-20`, as Codex and Claude Code write them). */
export function linkTarget(href: string, cwd: string): string | null {
  if (/^https?:\/\//.test(href)) return href;
  if (/^[a-z][a-z0-9+.-]*:/i.test(href) && !href.startsWith("file:")) return null;
  let path = decodeURI(href.replace(/^file:\/\//, "")).replace(/#.*$/, "");
  path = path.replace(/:\d+(:\d+)?(-\d+(:\d+)?)?$/, "");
  if (!path) return null;
  return path.startsWith("/") ? path : `${cwd.replace(/\/$/, "")}/${path.replace(/^\.\//, "")}`;
}

function text(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(text).join("");
  if (node && typeof node === "object" && "props" in node) return text((node.props as { children?: ReactNode }).children);
  return "";
}

function CopyButton({ value }: { value: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      className="icon-button"
      aria-label="Copy"
      data-tip={done ? "Copied" : "Copy"}
      onClick={() =>
        copyText(value).then(() => {
          setDone(true);
          setTimeout(() => setDone(false), 1200);
        })
      }
    >
      <Icon name={done ? "check" : "copy"} size={13} />
    </button>
  );
}

function CodeBlock({ lang, children }: { lang: string; children: ReactNode }) {
  const source = text(children).replace(/\n$/, "");
  if (lang === "mermaid") return <Diagram source={source} />;
  if (lang === "svg" && /^\s*<svg[\s>]/.test(source)) return <Svg source={source} />;
  return (
    <figure className="md-code">
      <figcaption>
        <span>{lang || "text"}</span>
        <CopyButton value={source} />
      </figcaption>
      <pre>
        <code className={lang ? `hljs language-${lang}` : "hljs"}>{children}</code>
      </pre>
    </figure>
  );
}

/** A figure with a "Source" toggle: the drawing, or what it was drawn from. */
function Figure({ kind, source, children }: { kind: string; source: string; children: ReactNode }) {
  const [raw, setRaw] = useState(false);
  return (
    <figure className="md-code md-figure">
      <figcaption>
        <span>{kind}</span>
        <button className="link small" onClick={() => setRaw(!raw)}>
          {raw ? "Show drawing" : "Show source"}
        </button>
        <CopyButton value={source} />
      </figcaption>
      {raw ? (
        <pre>
          <code>{source}</code>
        </pre>
      ) : (
        children
      )}
    </figure>
  );
}

const dark = () => window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
/** The app's palette for diagrams and charts (Mermaid reads hex, not OKLCH): one green for
 * shapes, quiet lines, and a set of even, muted hues for chart series. */
function themeVariables(isDark: boolean) {
  const series = isDark
    ? ["#5fb796", "#7f9fe0", "#e0a35c", "#a98bdb", "#e07f78", "#62bccb", "#b5bd5a", "#cf8bb8"]
    : ["#3f8f74", "#5b7fc4", "#c98232", "#8a6cc0", "#c4605a", "#3f97a8", "#8f9a34", "#ad6697"];
  const ink = isDark ? "#e4eaec" : "#26323a";
  const panel = isDark ? "#262d30" : "#fcfdfc";
  const pie = Object.fromEntries(series.map((c, i) => [`pie${i + 1}`, c]));
  return {
    darkMode: isDark,
    background: panel,
    fontSize: "13px",
    primaryColor: isDark ? "#1f3a33" : "#e5f1ec",
    primaryBorderColor: isDark ? "#5fb796" : "#5f9c86",
    primaryTextColor: ink,
    secondaryColor: isDark ? "#2b3340" : "#eaeff8",
    tertiaryColor: isDark ? "#2e2c25" : "#f6f1e8",
    lineColor: isDark ? "#8a99a3" : "#7d8a92",
    textColor: ink,
    mainBkg: isDark ? "#1f3a33" : "#e5f1ec",
    nodeBorder: isDark ? "#5fb796" : "#5f9c86",
    clusterBkg: isDark ? "#20272a" : "#f3f6f5",
    clusterBorder: isDark ? "#3a4448" : "#d5dcdb",
    edgeLabelBackground: panel,
    noteBkgColor: isDark ? "#2e2c25" : "#f6f1e8",
    noteBorderColor: isDark ? "#6b6250" : "#d9c9a8",
    actorBkg: isDark ? "#1f3a33" : "#e5f1ec",
    actorBorder: isDark ? "#5fb796" : "#5f9c86",
    ...pie,
    pieOpacity: "1",
    pieStrokeColor: panel,
    pieStrokeWidth: "2px",
    pieOuterStrokeWidth: "0px",
    pieSectionTextColor: "#ffffff",
    pieTitleTextSize: "15px",
    pieLegendTextColor: ink,
    pieTitleTextColor: ink,
    xyChart: { plotColorPalette: series.join(","), backgroundColor: panel, titleColor: ink, xAxisLabelColor: ink, yAxisLabelColor: ink, xAxisLineColor: isDark ? "#4a555a" : "#c9d1d3", yAxisLineColor: isDark ? "#4a555a" : "#c9d1d3", xAxisTickColor: isDark ? "#4a555a" : "#c9d1d3", yAxisTickColor: isDark ? "#4a555a" : "#c9d1d3", xAxisTitleColor: ink, yAxisTitleColor: ink },
  };
}

let mermaidReady: Promise<typeof import("mermaid").default> | null = null;
function loadMermaid() {
  mermaidReady ??= import("mermaid").then(({ default: m }) => {
    m.initialize({
      startOnLoad: false,
      // Strict: no scripts or click handlers in what the agent wrote; labels are sanitized.
      securityLevel: "strict",
      theme: "base",
      themeVariables: themeVariables(dark()),
      fontFamily: "Geist Variable, ui-sans-serif, system-ui, sans-serif",
    });
    return m;
  });
  return mermaidReady;
}

/** Mermaid diagrams and charts (flowchart, sequence, pie, xychart, gantt, …), drawn in the app. */
function Diagram({ source }: { source: string }) {
  const id = `mmd-${useId().replace(/[^a-zA-Z0-9]/g, "")}`;
  const [svg, setSvg] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let alive = true;
    loadMermaid()
      .then((m) => m.render(id, source))
      .then((r) => alive && (setSvg(r.svg), setFailed(false)))
      .catch(() => {
        // Mermaid leaves its error drawing in the page: take it away.
        document.getElementById(`d${id}`)?.remove();
        if (alive) setFailed(true);
      });
    return () => {
      alive = false;
    };
  }, [id, source]);
  if (failed)
    return (
      <figure className="md-code">
        <figcaption>
          <span>mermaid · couldn't draw this</span>
          <CopyButton value={source} />
        </figcaption>
        <pre>
          <code>{source}</code>
        </pre>
      </figure>
    );
  return (
    <Figure kind={/^\s*(pie|xychart|quadrantChart|sankey|gantt|radar)/.test(source) ? "chart" : "diagram"} source={source}>
      {svg ? <div className="md-diagram" dangerouslySetInnerHTML={{ __html: svg }} /> : <div className="md-diagram skeleton-block" aria-label="Drawing the diagram" />}
    </Figure>
  );
}

/** An SVG the agent wrote, shown as an image (an image can't run scripts). */
function Svg({ source }: { source: string }) {
  return (
    <Figure kind="svg" source={source}>
      <div className="md-diagram">
        <img alt="Drawing from the agent" src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(source)}`} />
      </div>
    </Figure>
  );
}

function components(cwd: string): Components {
  return {
    a: ({ href, children }) => {
      const target = href ? linkTarget(href, cwd) : null;
      return (
        <a
          href={href}
          title={target ?? href}
          onClick={(e) => {
            e.preventDefault();
            if (target) openTarget(target).catch(() => {});
          }}
        >
          {children}
        </a>
      );
    },
    pre: ({ children }) => {
      const code = Array.isArray(children) ? children[0] : children;
      const cls = (code as { props?: { className?: string } })?.props?.className ?? "";
      const lang = /language-([\w+-]+)/.exec(cls)?.[1]?.toLowerCase() ?? "";
      return <CodeBlock lang={lang}>{(code as { props?: { children?: ReactNode } })?.props?.children}</CodeBlock>;
    },
    table: ({ children }) => (
      <div className="md-table">
        <table>{children}</table>
      </div>
    ),
    // Pictures from the web aren't loaded (the app stays offline-safe): a link instead.
    img: ({ src, alt }) =>
      typeof src === "string" && src.startsWith("data:image/") ? (
        <img src={src} alt={alt ?? ""} />
      ) : (
        <a
          href={typeof src === "string" ? src : undefined}
          onClick={(e) => {
            e.preventDefault();
            const t = typeof src === "string" ? linkTarget(src, cwd) : null;
            if (t) openTarget(t).catch(() => {});
          }}
        >
          {alt || "image"}
        </a>
      ),
    input: ({ checked }) => <input type="checkbox" checked={!!checked} readOnly tabIndex={-1} />,
  };
}

/** What the agent wrote, as Markdown (GitHub's flavour). */
export const Markdown = memo(function Markdown({ source, cwd }: { source: string; cwd: string }) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[[rehypeHighlight, { detect: false }]]} components={components(cwd)}>
        {source}
      </ReactMarkdown>
    </div>
  );
});
