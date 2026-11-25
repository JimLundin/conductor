import type { FunctionComponent } from "react";
import type { Offer } from "@conductor/core/session/commands";
import { OfferButton } from "#ui/session/offer-button.tsx";

interface Props {
  readonly disabled: boolean;
  readonly offers: ReadonlyArray<Offer>;
  readonly run: (body: Offer["body"]) => void;
}

const Offers: FunctionComponent<Props> = ({ disabled, offers, run }) => (
  <div className="offers">
    {offers.map((offer) => (
      <OfferButton disabled={disabled} key={offer.label} offer={offer} run={run} />
    ))}
  </div>
);

export { Offers };
