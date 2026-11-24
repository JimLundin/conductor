// The text of a 128-bit id, as the `ulid` crate writes it: 26 digits of Crockford's base 32 in
// upper case. Hosts write that and read either case, and the app compares ids as text, so it
// reads every id it is handed into that form. The generated schemas type these ids as plain
// strings, so this is the app's one codec of their text.
import { Effect, Random } from "effect";

const crockford = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const length = 26;
// 128 bits in 26 digits leave the first digit only 3 of them.
const firstDigits = 8;

const digit = (below: number): Effect.Effect<string> =>
	Random.nextIntBetween(0, below, { halfOpen: true }).pipe(
		Effect.map((value) => crockford.charAt(value)),
	);

// The id `text` names, in the form hosts write, or undefined when it names none.
const canonical = (text: string): string | undefined => {
	const upper = text.toUpperCase();
	const digits = [...upper].map((character) => crockford.indexOf(character));
	const [first] = digits;
	if (
		digits.length !== length ||
		digits.includes(-1) ||
		first === undefined ||
		first >= firstDigits
	) {
		return undefined;
	}
	return upper;
};

// 128 fresh random bits.
const mint: Effect.Effect<string> = Effect.all([
	digit(firstDigits),
	...Array.from({ length: length - 1 }, () => digit(crockford.length)),
]).pipe(Effect.map((all) => all.join("")));

export { canonical, mint };
