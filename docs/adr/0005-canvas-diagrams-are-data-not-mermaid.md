# Canvas diagrams are data, not Mermaid

A canvas's diagrams and statecharts are typed nodes, edges and states in the `home` crate, not Mermaid text, so the app can draw them as its own elements, `home canvas` can refuse one that breaks the drawing rules (six nodes a step, eight words a label), and the app decodes them with the schemas generated for the client link. Mermaid is only an output: `home canvas show` prints it for the terminal and GitHub.

## Rejected

- **Mermaid text in a block.** It renders anywhere today, but the app would have to parse Mermaid or hand it to Mermaid's renderer, a second definition of every diagram that no type checks, and the rules on a diagram's size could only be written in a skill.
