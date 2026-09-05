import { memo, useState, useDeferredValue } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { translate, type Locale } from "./i18n";

// Model output cannot load tracking images, execute HTML or navigate the WebView.
export const MessageContent = memo(function MessageContent({ text, locale }: { text: string; locale: Locale }) {
  const [linkError, setLinkError] = useState(false);
  const deferredText = useDeferredValue(text);
  return <div className="markdown-message">
    <Markdown skipHtml remarkPlugins={[remarkGfm]} components={{
      img: ({ alt }) => <span>{alt}</span>,
      table: ({ children }) => <div className="markdown-table-scroll"><table>{children}</table></div>,
      a: ({ href, children }) => {
        let url: URL;
        try { url = new URL(href ?? ""); } catch { return <span>{children}</span>; }
        if (url.protocol !== "https:" || url.username || url.password) return <span>{children}</span>;
        return <a href={url.href} title={url.hostname} rel="noreferrer noopener" onClick={(event) => {
          event.preventDefault();
          setLinkError(false);
          void openUrl(url.href).catch(() => setLinkError(true));
        }}>{children}<span className="external-link-mark" aria-hidden="true"> ↗</span></a>;
      },
    }}>{deferredText}</Markdown>
    {linkError && <small role="alert">{translate(locale, "sourceOpenFailed")}</small>}
  </div>;
});
