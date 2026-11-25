interface Block {
  readonly key: string;
  readonly kind: "Paragraph" | "Code";
  readonly text: string;
}

interface Piece {
  readonly key: string;
  readonly tag: "code" | "span";
  readonly text: string;
}

const fence = "```";
const first = 0;
const second = 1;
const alternating = 2;

const blocks = (text: string): ReadonlyArray<Block> =>
  text.split(fence).flatMap((part, position): ReadonlyArray<Block> => {
    if (position % alternating === second) {
      const body = part.slice(part.indexOf("\n") + second).replace(/\n$/u, "");
      return [{ key: String(position), kind: "Code", text: body }];
    }
    return part
      .split(/\n{2,}/u)
      .map((paragraph) => paragraph.trim())
      .filter((paragraph) => paragraph.length > first)
      .map((paragraph, index) => ({
        key: `${String(position)}-${String(index)}`,
        kind: "Paragraph",
        text: paragraph,
      }));
  });

const pieces = (text: string): ReadonlyArray<Piece> =>
  text.split("`").map((piece, position) => ({
    key: String(position),
    tag: position % alternating === second ? "code" : "span",
    text: piece,
  }));

export { blocks, pieces };
