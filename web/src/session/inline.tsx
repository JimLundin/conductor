import type { FunctionComponent } from "react";
import { pieces } from "@conductor/core/view/markdown";

const Inline: FunctionComponent<{ readonly text: string }> = ({ text }) => (
  <>
    {pieces(text).map(({ key, tag: Tag, text: piece }) => (
      <Tag key={key}>{piece}</Tag>
    ))}
  </>
);

export { Inline };
