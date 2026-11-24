// An agent's text as paragraphs, fenced code and inline code: the markdown its replies lean on.
import type { ReactElement } from "react";

type Block =
  | { readonly kind: "Paragraph"; readonly text: string }
  | { readonly kind: "Code"; readonly text: string };

const fence = "```";

// Text between fences is code; a fence still open, as while a reply streams, runs to the end.
const blocks = (text: string): readonly Block[] =>
  text.split(fence).flatMap((part, at): readonly Block[] => {
    if (at % 2 === 1) {
      const body = part.slice(part.indexOf("\n") + 1).replace(/\n$/u, "");
      return [{ kind: "Code", text: body }];
    }
    return part
      .split(/\n{2,}/u)
      .map((paragraph) => paragraph.trim())
      .filter((paragraph) => paragraph.length > 0)
      .map((paragraph) => ({ kind: "Paragraph", text: paragraph }));
  });

// Odd pieces between backticks are inline code.
const Inline = ({ text }: { readonly text: string }): ReactElement => (
  <>
    {text
      .split("`")
      .map((piece, at) => ({ piece, isCode: at % 2 === 1, key: `${at}` }))
      .map(({ piece, isCode, key }) =>
        isCode ? <code key={key}>{piece}</code> : <span key={key}>{piece}</span>,
      )}
  </>
);

const Markdown = ({ text }: { readonly text: string }): ReactElement => (
  <div className="markdown">
    {blocks(text)
      .map((block, at) => ({ block, key: `${at}` }))
      .map(({ block, key }) =>
        block.kind === "Code" ? (
          <pre key={key}>
            <code>{block.text}</code>
          </pre>
        ) : (
          <p key={key}>
            <Inline text={block.text} />
          </p>
        ),
      )}
  </div>
);

export { Markdown };
