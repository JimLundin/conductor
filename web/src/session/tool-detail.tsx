import type { FunctionComponent } from "react";
import type { ToolKind } from "@conductor/core/schemas/index";

const ToolDetail: FunctionComponent<{ readonly tool: ToolKind }> = ({ tool }) => {
  if ("Command" in tool) {
    return <pre className="command">{tool.Command.command}</pre>;
  }
  if ("FileChange" in tool) {
    return (
      <ul className="files">
        {tool.FileChange.files.map((file) => (
          <li key={`${file.change} ${file.path}`}>
            <span className={`change ${file.change.toLowerCase()}`}>{file.change}</span>
            <code>{file.path}</code>
          </li>
        ))}
      </ul>
    );
  }
  return <pre className="command">{tool.Other.detail}</pre>;
};

export { ToolDetail };
