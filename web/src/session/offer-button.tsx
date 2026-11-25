import type { FunctionComponent } from "react";
import type { Offer } from "@conductor/core/session/commands";
import { useCallback } from "react";

interface Props {
  readonly disabled: boolean;
  readonly offer: Offer;
  readonly run: (body: Offer["body"]) => void;
}

const OfferButton: FunctionComponent<Props> = ({ disabled, offer, run }) => {
  const click = useCallback((): void => {
    run(offer.body);
  }, [offer.body, run]);
  return (
    <button className={offer.tone.toLowerCase()} disabled={disabled} onClick={click} type="button">
      {offer.label}
    </button>
  );
};

export { OfferButton };
