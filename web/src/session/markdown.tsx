import type { FunctionComponent } from "react";
import { Inline } from "#ui/session/inline.tsx";
import { blocks } from "@conductor/core/view/markdown";

const Markdown: FunctionComponent<{ readonly text: string }> = ({ text }) => (
  <div className="markdown">
    {blocks(text).map((block) =>
      block.kind === "Code" ? (
        <pre key={block.key}>
          <code>{block.text}</code>
        </pre>
      ) : (
        <p key={block.key}>
          <Inline text={block.text} />
        </p>
      ),
    )}
  </div>
);

export { Markdown };
