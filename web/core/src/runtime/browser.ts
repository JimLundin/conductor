import { Schema } from "effect";

const Visible = Schema.Struct({ visibilityState: Schema.Literal("visible") });
const isVisible = Schema.is(Visible);

export { isVisible };
