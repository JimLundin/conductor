import type { FunctionComponent } from "react";
import type { MarkerItem } from "@conductor/core/view/transcript";
import { marker } from "@conductor/core/view/transcript";

const Marker: FunctionComponent<{ readonly item: MarkerItem }> = ({ item }) => {
  const shown = marker(item);
  return <p className={`marker ${shown.tone}`}>{shown.text}</p>;
};

export { Marker };
