import { translate, type Locale } from "./i18n";

export function ToolActivity({ items, locale }: { items: string[]; locale: Locale }) {
  const tx = (key: Parameters<typeof translate>[1], values?: Record<string, string | number>) => translate(locale, key, values);
  return <details className="tool-timeline">
    <summary>{tx("toolActivity")} <span className="tool-count">{items.length}</span></summary>
    <ol>{items.map((item, index) => {
      const separator = item.indexOf(" · ");
      const name = separator < 0 ? item : item.slice(0, separator);
      const detail = separator < 0 ? "" : item.slice(separator + 3);
      const status = detail === "running" ? tx("toolRunning")
        : detail === "completed" ? tx("toolCompleted")
        : detail === "unsupported" ? tx("toolUnsupported")
        : detail.startsWith("completed_sources:") ? tx("toolCompletedSources", { count: detail.split(":")[1] })
        : detail.startsWith("retry:") ? tx("toolRetry", { count: detail.split(":")[1] }) : detail;
      return <li key={index} data-phase={detail.split(":")[0]}><code>{name}</code><span>{status}</span></li>;
    })}</ol>
  </details>;
}
