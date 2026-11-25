import type { FunctionComponent } from "react";

interface Props {
  readonly text: string | undefined;
  readonly tone: string;
  readonly role?: "alert";
}

const Notice: FunctionComponent<Props> = ({ role, text, tone }) =>
  typeof text === "string" ? (
    <p className={tone} role={role}>
      {text}
    </p>
  ) : undefined;

export { Notice };
