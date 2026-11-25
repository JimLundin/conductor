# Canvas diagrams are data, not Mermaid

A canvas's diagrams and statecharts are typed nodes, edges and states in the `home` crate, not Mermaid text. The app can then draw them as its own elements and decode them with the schemas generated for the client link. `home canvas` can refuse a diagram that breaks the drawing rules, which allow six nodes a step and eight words a label. Mermaid is only an output, which `home canvas show` prints for the terminal and GitHub.

## Rejected

- **Mermaid text in a block.** It renders anywhere today. But the app would have to parse Mermaid or hand it to Mermaid's renderer, and the text would be a second definition of every diagram that no type checks. The rules on a diagram's size could then live only in a skill.
