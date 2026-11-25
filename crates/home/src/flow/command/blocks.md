# Canvas blocks

A canvas is one JSON object with a `title` and a list of `blocks`. Each block is an object with one key, the block's kind. That key's value holds the block's fields. A field is required unless its section calls it optional.

```json
{
  "title": "How a session holds the lock",
  "blocks": [
    {"prose": {"text": "The lock now lives in the store."}},
    {"code": {"path": "src/lock.rs"}}
  ]
}
```

## `prose`

A `prose` block holds a paragraph or two of markdown in its `text`.

```json
{"prose": {"text": "The lock now lives in the store, so two sessions can't both write."}}
```

## `diagram`

A `diagram` block holds nodes, the edges between them, and groups that each draw a box around some nodes. Each node and each edge has a `step`, counting from 1. A step shows its own nodes and edges and everything from the steps before it. A node's `group` and an edge's `label` are optional. Give `groups` as `[]` when there are none.

```json
{"diagram": {
  "nodes": [
    {"id": "agent", "label": "the agent", "step": 1},
    {"id": "put", "label": "home canvas put", "group": "home", "step": 1},
    {"id": "store", "label": "the canvas store", "group": "home", "step": 2}
  ],
  "edges": [
    {"from": "agent", "to": "put", "label": "sends the JSON", "step": 1},
    {"from": "put", "to": "store", "step": 2}
  ],
  "groups": [{"id": "home", "label": "the home command"}]
}}
```

## `statechart`

A `statechart` block holds states, the state it starts in, and the transitions between them. Each transition names the `event` that causes it. A state's optional `marked` gives a short reason the state matters, drawn as a note beside it. The optional `path` lists the states of one walk to highlight, such as the usual run.

```json
{"statechart": {
  "states": [
    {"id": "open", "label": "open"},
    {"id": "held", "label": "held", "marked": "only one session at a time"},
    {"id": "done", "label": "done"}
  ],
  "start": "open",
  "transitions": [
    {"from": "open", "to": "held", "event": "a session takes it"},
    {"from": "held", "to": "open", "event": "the session lets go"},
    {"from": "held", "to": "done", "event": "the write finishes"}
  ],
  "path": ["open", "held", "done"]
}}
```

## `options`

An `options` block holds a question, the options weighed, and the index of the option you pick in `pick`. Each option says what it gets and what it costs. The `why` says why the pick wins.

```json
{"options": {
  "question": "Where does the lock live?",
  "options": [
    {"label": "in the store", "gets": "one place to look", "costs": "a round trip per write"},
    {"label": "in each session", "gets": "no round trip", "costs": "two sessions can both think they hold it"}
  ],
  "pick": 0,
  "why": "A lost write costs more than a round trip."
}}
```

## `changes`

A `changes` block holds a diff as groups of entries. Each group's `kind` is one of these:

- **`core`**: the logic that decides behaviour.
- **`wiring`**: the code that connects the core to the rest.
- **`mechanical`**: renames, moves and generated code.

Each entry points at code the way a `code` block does, and its `note` says in a line what changed there. An entry's `tag` is optional. Tag only a change that is `subtle`, `breaking`, a `race` or a `perf` cost.

```json
{"changes": {"groups": [
  {"kind": "core", "entries": [
    {"code": {"path": "src/lock.rs", "lines": [12, 40]}, "note": "takes the lock before a write", "tag": "race"}
  ]},
  {"kind": "mechanical", "entries": [
    {"code": {"path": "src/lib.rs"}, "note": "exports the lock"}
  ]}
]}}
```

## `code`

A `code` block points at a place in the code. Its `path` runs from the repository's root. Its optional `lines` gives the first and last line, and its optional `commit` names a commit.

```json
{"code": {"path": "src/lock.rs", "lines": [12, 40], "commit": "e77b8213"}}
```

## `progress`

A `progress` block holds a `label` and a `bar`. The bar is a `count` of how many are done out of a total, or `stretches` for a bar in parts, each part with its own label.

```json
{"progress": {"label": "frames matched", "bar": {"count": {"done": 3, "total": 8}}}}
```

```json
{"progress": {"label": "scenes matched", "bar": {"stretches": [
  {"label": "the opening", "done": 4, "total": 4},
  {"label": "the chase", "done": 1, "total": 6}
]}}}
```
