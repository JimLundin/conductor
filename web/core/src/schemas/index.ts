import { Schema } from "effect";

type Account = "Own" | { readonly Named: string };

const Account = Schema.Union([Schema.Literal("Own"), Schema.Struct({ Named: Schema.String })], {
  mode: "oneOf",
}).annotate({ identifier: "Account" });

type Alpn = "conductor/50";

const Alpn = Schema.Literal("conductor/50").annotate({
  description: "The ALPN a client dials a host with.",
  identifier: "Alpn",
});

type Answer = globalThis.ReadonlyArray<string>;

const Answer = Schema.Array(Schema.String).annotate({ identifier: "Answer" });

type Auth =
  | "SignedIn"
  | "Unmanaged"
  | { readonly Stored: { readonly kind: string; readonly url: string | null } };

const Auth = Schema.Union(
  [
    Schema.Literals(["SignedIn", "Unmanaged"]),
    Schema.Struct({
      Stored: Schema.Struct({
        kind: Schema.String,
        url: Schema.Union([Schema.String, Schema.Null]),
      }),
    }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Auth" });

type AuthToken = globalThis.ReadonlyArray<number>;

const AuthToken = Schema.Array(
  Schema.Number.annotate({ format: "uint8" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    )
    .check(
      Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
    ),
)
  .check(Schema.isMinLength(16).annotate({ expected: "a value with a length of at least 16" }))
  .check(
    Schema.isMaxLength(16).annotate({
      expected: "a value with a length of at most 16",
      identifier: "AuthToken",
    }),
  );

type Effort = "Low" | "Medium" | "High" | "XHigh" | "Max" | "Ultra";

const Effort = Schema.Literals(["Low", "Medium", "High", "XHigh", "Max", "Ultra"]).annotate({
  identifier: "Effort",
});

type ModelId = string;

const ModelId = Schema.String.annotate({ identifier: "ModelId" });

type ModelChoice = {
  readonly description: string;
  readonly efforts: globalThis.ReadonlyArray<Effort>;
  readonly id: ModelId;
  readonly name: string;
};

const ModelChoice = Schema.Struct({
  description: Schema.String,
  efforts: Schema.Array(Effort),
  id: ModelId,
  name: Schema.String,
}).annotate({ identifier: "ModelChoice" });

type Models = {
  readonly choices: globalThis.ReadonlyArray<ModelChoice>;
  readonly default: ModelId | null;
};

const Models = Schema.Struct({
  choices: Schema.Array(ModelChoice),
  default: Schema.Union([ModelId, Schema.Null]),
}).annotate({ identifier: "Models" });

type Availability =
  | { readonly Available: { readonly auth: Auth; readonly models: Models } }
  | { readonly Unavailable: { readonly reason: string } };

const Availability = Schema.Union(
  [
    Schema.Struct({ Available: Schema.Struct({ auth: Auth, models: Models }) }),
    Schema.Struct({ Unavailable: Schema.Struct({ reason: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Availability" });

type Stretch = { readonly done: number; readonly label: string; readonly total: number };

const Stretch = Schema.Struct({
  done: Schema.Number.annotate({ format: "uint64" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
  label: Schema.String,
  total: Schema.Number.annotate({ format: "uint64" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
}).annotate({ identifier: "Stretch" });

type Bar =
  | { readonly count: { readonly done: number; readonly total: number } }
  | { readonly stretches: globalThis.ReadonlyArray<Stretch> };

const Bar = Schema.Union(
  [
    Schema.Struct({
      count: Schema.Struct({
        done: Schema.Number.annotate({ format: "uint64" })
          .check(Schema.isInt().annotate({ expected: "an integer" }))
          .check(
            Schema.isGreaterThanOrEqualTo(0).annotate({
              expected: "a value greater than or equal to 0",
            }),
          ),
        total: Schema.Number.annotate({ format: "uint64" })
          .check(Schema.isInt().annotate({ expected: "an integer" }))
          .check(
            Schema.isGreaterThanOrEqualTo(0).annotate({
              expected: "a value greater than or equal to 0",
            }),
          ),
      }),
    }),
    Schema.Struct({ stretches: Schema.Array(Stretch) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Bar" });

type Edge = {
  readonly from: string;
  readonly label?: string | null;
  readonly step: number;
  readonly to: string;
};

const Edge = Schema.Struct({
  from: Schema.String,
  label: Schema.optionalKey(Schema.Union([Schema.String, Schema.Null])),
  step: Schema.Number.annotate({ format: "uint8" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    )
    .check(
      Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
    ),
  to: Schema.String,
}).annotate({ identifier: "Edge" });

type Group = { readonly id: string; readonly label: string };

const Group = Schema.Struct({ id: Schema.String, label: Schema.String }).annotate({
  identifier: "Group",
});

type Node = {
  readonly group?: string | null;
  readonly id: string;
  readonly label: string;
  readonly step: number;
};

const Node = Schema.Struct({
  group: Schema.optionalKey(Schema.Union([Schema.String, Schema.Null])),
  id: Schema.String,
  label: Schema.String,
  step: Schema.Number.annotate({ format: "uint8" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    )
    .check(
      Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
    ),
}).annotate({ identifier: "Node" });

type Diagram = {
  readonly edges: globalThis.ReadonlyArray<Edge>;
  readonly groups: globalThis.ReadonlyArray<Group>;
  readonly nodes: globalThis.ReadonlyArray<Node>;
};

const Diagram = Schema.Struct({
  edges: Schema.Array(Edge),
  groups: Schema.Array(Group),
  nodes: Schema.Array(Node),
}).annotate({ identifier: "Diagram" });

type State = { readonly id: string; readonly label: string; readonly marked?: string | null };

const State = Schema.Struct({
  id: Schema.String,
  label: Schema.String,
  marked: Schema.optionalKey(Schema.Union([Schema.String, Schema.Null])),
}).annotate({ identifier: "State" });

type Transition = { readonly event: string; readonly from: string; readonly to: string };

const Transition = Schema.Struct({
  event: Schema.String,
  from: Schema.String,
  to: Schema.String,
}).annotate({ identifier: "Transition" });

type Statechart = {
  readonly path?: globalThis.ReadonlyArray<string>;
  readonly start: string;
  readonly states: globalThis.ReadonlyArray<State>;
  readonly transitions: globalThis.ReadonlyArray<Transition>;
};

const Statechart = Schema.Struct({
  path: Schema.optionalKey(Schema.Array(Schema.String)),
  start: Schema.String,
  states: Schema.Array(State),
  transitions: Schema.Array(Transition),
}).annotate({ identifier: "Statechart" });

type Progress = { readonly bar: Bar; readonly label: string };

const Progress = Schema.Struct({ bar: Bar, label: Schema.String }).annotate({
  identifier: "Progress",
});

type Choice = { readonly costs: string; readonly gets: string; readonly label: string };

const Choice = Schema.Struct({
  costs: Schema.String,
  gets: Schema.String,
  label: Schema.String,
}).annotate({ identifier: "Choice" });

type Options = {
  readonly options: globalThis.ReadonlyArray<Choice>;
  readonly pick: number;
  readonly question: string;
  readonly why: string;
};

const Options = Schema.Struct({
  options: Schema.Array(Choice),
  pick: Schema.Number.annotate({ format: "uint" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
  question: Schema.String,
  why: Schema.String,
}).annotate({ identifier: "Options" });

type CodeRef = {
  readonly commit?: string | null;
  readonly lines?: readonly [number, number] | null;
  readonly path: string;
};

const CodeRef = Schema.Struct({
  commit: Schema.optionalKey(Schema.Union([Schema.String, Schema.Null])),
  lines: Schema.optionalKey(
    Schema.Union([
      Schema.Tuple([
        Schema.Number.annotate({ format: "uint32" })
          .check(Schema.isInt().annotate({ expected: "an integer" }))
          .check(
            Schema.isGreaterThanOrEqualTo(0).annotate({
              expected: "a value greater than or equal to 0",
            }),
          ),
        Schema.Number.annotate({ format: "uint32" })
          .check(Schema.isInt().annotate({ expected: "an integer" }))
          .check(
            Schema.isGreaterThanOrEqualTo(0).annotate({
              expected: "a value greater than or equal to 0",
            }),
          ),
      ]),
      Schema.Null,
    ]),
  ),
  path: Schema.String,
}).annotate({ identifier: "CodeRef" });

type Tag = "subtle" | "breaking" | "race" | "perf";

const Tag = Schema.Literals(["subtle", "breaking", "race", "perf"]).annotate({ identifier: "Tag" });

type Change = { readonly code: CodeRef; readonly note: string; readonly tag?: Tag | null };

const Change = Schema.Struct({
  code: CodeRef,
  note: Schema.String,
  tag: Schema.optionalKey(Schema.Union([Tag, Schema.Null])),
}).annotate({ identifier: "Change" });

type ChangeKind = "core" | "wiring" | "mechanical";

const ChangeKind = Schema.Literals(["core", "wiring", "mechanical"]).annotate({
  identifier: "ChangeKind",
});

type ChangeGroup = {
  readonly entries: globalThis.ReadonlyArray<Change>;
  readonly kind: ChangeKind;
};

const ChangeGroup = Schema.Struct({ entries: Schema.Array(Change), kind: ChangeKind }).annotate({
  identifier: "ChangeGroup",
});

type Changes = { readonly groups: globalThis.ReadonlyArray<ChangeGroup> };

const Changes = Schema.Struct({ groups: Schema.Array(ChangeGroup) }).annotate({
  identifier: "Changes",
});

type Block =
  | { readonly prose: { readonly text: string } }
  | { readonly diagram: Diagram }
  | { readonly statechart: Statechart }
  | { readonly progress: Progress }
  | { readonly options: Options }
  | { readonly changes: Changes }
  | { readonly code: CodeRef };

const Block = Schema.Union(
  [
    Schema.Struct({ prose: Schema.Struct({ text: Schema.String }) }),
    Schema.Struct({ diagram: Diagram }),
    Schema.Struct({ statechart: Statechart }),
    Schema.Struct({ progress: Progress }),
    Schema.Struct({ options: Options }),
    Schema.Struct({ changes: Changes }),
    Schema.Struct({ code: CodeRef }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Block" });

type Body = string;

const Body = Schema.String.annotate({ identifier: "Body" });

type Canvas = { readonly blocks: globalThis.ReadonlyArray<Block>; readonly title: string };

const Canvas = Schema.Struct({ blocks: Schema.Array(Block), title: Schema.String }).annotate({
  identifier: "Canvas",
});

type CanvasName = string;

const CanvasName = Schema.String.annotate({ identifier: "CanvasName" });

type CanvasEntry = { readonly by: string; readonly name: CanvasName; readonly written: number };

const CanvasEntry = Schema.Struct({
  by: Schema.String,
  name: CanvasName,
  written: Schema.Number.annotate({ format: "uint64" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
}).annotate({ identifier: "CanvasEntry" });

type CanvasFile = { readonly by: string; readonly canvas: Canvas; readonly written: number };

const CanvasFile = Schema.Struct({
  by: Schema.String,
  canvas: Canvas,
  written: Schema.Number.annotate({ format: "uint64" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
}).annotate({ identifier: "CanvasFile" });

type FieldKind = "Text" | "Url" | "Secret";

const FieldKind = Schema.Literals(["Text", "Url", "Secret"]).annotate({ identifier: "FieldKind" });

type Field = {
  readonly id: string;
  readonly kind: FieldKind;
  readonly label: string;
  readonly placeholder: string;
  readonly required: boolean;
};

const Field = Schema.Struct({
  id: Schema.String,
  kind: FieldKind,
  label: Schema.String,
  placeholder: Schema.String,
  required: Schema.Boolean,
}).annotate({ identifier: "Field" });

type CredentialForm = {
  readonly account: string;
  readonly fields: globalThis.ReadonlyArray<Field>;
  readonly kind: string;
  readonly label: string;
  readonly setup_sign_in: string | null;
};

const CredentialForm = Schema.Struct({
  account: Schema.String,
  fields: Schema.Array(Field),
  kind: Schema.String,
  label: Schema.String,
  setup_sign_in: Schema.Union([Schema.String, Schema.Null]),
}).annotate({ identifier: "CredentialForm" });

type ProviderInfo = {
  readonly credentials: globalThis.ReadonlyArray<CredentialForm>;
  readonly label: string;
  readonly note: string | null;
};

const ProviderInfo = Schema.Struct({
  credentials: Schema.Array(CredentialForm),
  label: Schema.String,
  note: Schema.Union([Schema.String, Schema.Null]),
}).annotate({ identifier: "ProviderInfo" });

type Catalog = { readonly [x: string]: ProviderInfo };

const Catalog = Schema.Record(Schema.String, ProviderInfo).annotate({ identifier: "Catalog" });

type Change2 = "Write" | "Edit" | "Delete";

const Change2 = Schema.Literals(["Write", "Edit", "Delete"]).annotate({ identifier: "Change2" });

type ChecklistItem = { readonly done: boolean; readonly text: string };

const ChecklistItem = Schema.Struct({ done: Schema.Boolean, text: Schema.String }).annotate({
  identifier: "ChecklistItem",
});

type Item = {
  readonly index: number;
  readonly seen: globalThis.ReadonlyArray<ChecklistItem> | null;
};

const Item = Schema.Struct({
  index: Schema.Number.annotate({ format: "uint" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
  seen: Schema.Union([Schema.Array(ChecklistItem), Schema.Null]),
}).annotate({ identifier: "Item" });

type ChecklistChange =
  | { readonly Add: string }
  | { readonly Check: { readonly done: boolean; readonly item: Item } }
  | { readonly Remove: Item };

const ChecklistChange = Schema.Union(
  [
    Schema.Struct({ Add: Schema.String }),
    Schema.Struct({ Check: Schema.Struct({ done: Schema.Boolean, item: Item }) }),
    Schema.Struct({ Remove: Item }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "ChecklistChange" });

type ChecklistStatus = "Pending" | "InProgress" | "Completed";

const ChecklistStatus = Schema.Literals(["Pending", "InProgress", "Completed"]).annotate({
  identifier: "ChecklistStatus",
});

type ChecklistEntry = { readonly status: ChecklistStatus; readonly text: string };

const ChecklistEntry = Schema.Struct({ status: ChecklistStatus, text: Schema.String }).annotate({
  identifier: "ChecklistEntry",
});

type Origin = "Found" | "Cloned";

const Origin = Schema.Literals(["Found", "Cloned"]).annotate({ identifier: "Origin" });

type CheckoutLocation = string;

const CheckoutLocation = Schema.String.annotate({ identifier: "CheckoutLocation" });

type Checkout = { readonly origin: Origin; readonly path: CheckoutLocation };

const Checkout = Schema.Struct({ origin: Origin, path: CheckoutLocation }).annotate({
  identifier: "Checkout",
});

type ClientName = string;

const ClientName = Schema.String.annotate({ identifier: "ClientName" });

type Cloning = "Idle" | "Running" | { readonly Failed: { readonly reason: string } };

const Cloning = Schema.Union(
  [
    Schema.Literals(["Idle", "Running"]),
    Schema.Struct({ Failed: Schema.Struct({ reason: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Cloning" });

type CloseReason =
  | { readonly FailedToStart: { readonly message: string } }
  | { readonly Exited: { readonly status: string } }
  | { readonly Stopped: { readonly reason: string } };

const CloseReason = Schema.Union(
  [
    Schema.Struct({ FailedToStart: Schema.Struct({ message: Schema.String }) }),
    Schema.Struct({ Exited: Schema.Struct({ status: Schema.String }) }),
    Schema.Struct({ Stopped: Schema.Struct({ reason: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "CloseReason" });

type SessionDirectory = string;

const SessionDirectory = Schema.String.annotate({ identifier: "SessionDirectory" });

type ProjectId = string;

const ProjectId = Schema.String.check(
  Schema.isMinCodePoints(6).annotate({ expected: "a string with at least 6 code points" }),
).check(
  Schema.isMaxCodePoints(6).annotate({
    expected: "a string with at most 6 code points",
    identifier: "ProjectId",
  }),
);

type IssueId = string;

const IssueId = Schema.String.check(
  Schema.isPattern(
    new globalThis.RegExp("^(?:[A-Za-z0-9_-]|%[0-9A-F]{2})+~(?:[A-Za-z0-9_-]|%[0-9A-F]{2})+$", "u"),
  ).annotate({
    expected:
      "a string matching the RegExp ^(?:[A-Za-z0-9_-]|%[0-9A-F]{2})+~(?:[A-Za-z0-9_-]|%[0-9A-F]{2})+$",
    identifier: "IssueId",
  }),
);

type StartAt =
  | { readonly Directory: SessionDirectory }
  | { readonly Project: ProjectId }
  | { readonly Issue: IssueId };

const StartAt = Schema.Union(
  [
    Schema.Struct({ Directory: SessionDirectory }),
    Schema.Struct({ Project: ProjectId }),
    Schema.Struct({ Issue: IssueId }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "StartAt" });

type InstanceId = { readonly account: Account; readonly provider: string };

const InstanceId = Schema.Struct({ account: Account, provider: Schema.String }).annotate({
  identifier: "InstanceId",
});

type SessionId = string;

const SessionId = Schema.String.annotate({ identifier: "SessionId" });

type Decision = "Approve" | "ApproveForSession" | "Deny";

const Decision = Schema.Literals(["Approve", "ApproveForSession", "Deny"]).annotate({
  identifier: "Decision",
});

type RequestId = number;

const RequestId = Schema.Number.annotate({ format: "uint64" })
  .check(Schema.isInt().annotate({ expected: "an integer" }))
  .check(
    Schema.isGreaterThanOrEqualTo(0).annotate({
      expected: "a value greater than or equal to 0",
      identifier: "RequestId",
    }),
  );

type HostSeq = number;

const HostSeq = Schema.Number.annotate({ format: "uint64" })
  .check(Schema.isInt().annotate({ expected: "an integer" }))
  .check(
    Schema.isGreaterThanOrEqualTo(0).annotate({
      expected: "a value greater than or equal to 0",
      identifier: "HostSeq",
    }),
  );

type SessionCommand =
  | {
      readonly Start: {
        readonly at: StartAt;
        readonly effort: Effort | null;
        readonly instance: InstanceId;
        readonly model: ModelId | null;
      };
    }
  | { readonly SendMessage: { readonly session: SessionId; readonly text: string } }
  | { readonly Interrupt: { readonly session: SessionId } }
  | { readonly Continue: { readonly session: SessionId } }
  | {
      readonly Resolve: {
        readonly decision: Decision;
        readonly request: RequestId;
        readonly session: SessionId;
      };
    }
  | {
      readonly Answer: {
        readonly answers: globalThis.ReadonlyArray<Answer>;
        readonly request: RequestId;
        readonly session: SessionId;
      };
    }
  | { readonly GetDiff: { readonly session: SessionId; readonly snapshot: HostSeq } }
  | { readonly SetEffort: { readonly effort: Effort; readonly session: SessionId } }
  | { readonly Remove: { readonly session: SessionId } }
  | { readonly Finish: { readonly session: SessionId } };

const SessionCommand = Schema.Union(
  [
    Schema.Struct({
      Start: Schema.Struct({
        at: StartAt,
        effort: Schema.Union([Effort, Schema.Null]),
        instance: InstanceId,
        model: Schema.Union([ModelId, Schema.Null]),
      }),
    }),
    Schema.Struct({ SendMessage: Schema.Struct({ session: SessionId, text: Schema.String }) }),
    Schema.Struct({ Interrupt: Schema.Struct({ session: SessionId }) }),
    Schema.Struct({ Continue: Schema.Struct({ session: SessionId }) }),
    Schema.Struct({
      Resolve: Schema.Struct({ decision: Decision, request: RequestId, session: SessionId }),
    }),
    Schema.Struct({
      Answer: Schema.Struct({
        answers: Schema.Array(Answer),
        request: RequestId,
        session: SessionId,
      }),
    }),
    Schema.Struct({ GetDiff: Schema.Struct({ session: SessionId, snapshot: HostSeq }) }),
    Schema.Struct({ SetEffort: Schema.Struct({ effort: Effort, session: SessionId }) }),
    Schema.Struct({ Remove: Schema.Struct({ session: SessionId }) }),
    Schema.Struct({ Finish: Schema.Struct({ session: SessionId }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "SessionCommand" });

type Credential = { readonly kind: string; readonly values: { readonly [x: string]: string } };

const Credential = Schema.Struct({
  kind: Schema.String,
  values: Schema.Record(Schema.String, Schema.String),
}).annotate({ identifier: "Credential" });

type InstanceCommand =
  | { readonly SetCredential: { readonly credential: Credential; readonly provider: string } }
  | { readonly Remove: { readonly instance: InstanceId } };

const InstanceCommand = Schema.Union(
  [
    Schema.Struct({
      SetCredential: Schema.Struct({ credential: Credential, provider: Schema.String }),
    }),
    Schema.Struct({ Remove: Schema.Struct({ instance: InstanceId }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "InstanceCommand" });

type SignInCode = string;

const SignInCode = Schema.String.annotate({ identifier: "SignInCode" });

type SignInCommand =
  | { readonly Start: { readonly instance: InstanceId } }
  | { readonly SubmitCode: { readonly code: SignInCode; readonly instance: InstanceId } }
  | { readonly Cancel: { readonly instance: InstanceId } }
  | {
      readonly StartCredential: {
        readonly instance: InstanceId;
        readonly kind: string;
        readonly label: string;
      };
    };

const SignInCommand = Schema.Union(
  [
    Schema.Struct({ Start: Schema.Struct({ instance: InstanceId }) }),
    Schema.Struct({ SubmitCode: Schema.Struct({ code: SignInCode, instance: InstanceId }) }),
    Schema.Struct({ Cancel: Schema.Struct({ instance: InstanceId }) }),
    Schema.Struct({
      StartCredential: Schema.Struct({
        instance: InstanceId,
        kind: Schema.String,
        label: Schema.String,
      }),
    }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "SignInCommand" });

type EditId = string;

const EditId = Schema.String.annotate({ identifier: "EditId" });

type Datum = string | globalThis.ReadonlyArray<string> | boolean;

const Datum = Schema.Union([Schema.String, Schema.Array(Schema.String), Schema.Boolean]).annotate({
  identifier: "Datum",
});

type NewIssue = {
  readonly fields: globalThis.ReadonlyArray<readonly [string, Datum]>;
  readonly kind: string;
  readonly project: ProjectId;
  readonly title: string;
};

const NewIssue = Schema.Struct({
  fields: Schema.Array(Schema.Tuple([Schema.String, Datum])),
  kind: Schema.String,
  project: ProjectId,
  title: Schema.String,
}).annotate({ identifier: "NewIssue" });

type Part = "Stage" | "Status";

const Part = Schema.Literals(["Stage", "Status"]).annotate({ identifier: "Part" });

type IssueChange =
  | {
      readonly Move: {
        readonly part: Part;
        readonly reason: string;
        readonly signed_off: boolean;
        readonly to: string;
      };
    }
  | { readonly Set: { readonly field: string; readonly value: Datum } }
  | { readonly Title: string }
  | { readonly Note: string }
  | { readonly Body: string }
  | { readonly Checklist: ChecklistChange };

const IssueChange = Schema.Union(
  [
    Schema.Struct({
      Move: Schema.Struct({
        part: Part,
        reason: Schema.String,
        signed_off: Schema.Boolean,
        to: Schema.String,
      }),
    }),
    Schema.Struct({ Set: Schema.Struct({ field: Schema.String, value: Datum }) }),
    Schema.Struct({ Title: Schema.String }),
    Schema.Struct({ Note: Schema.String }),
    Schema.Struct({ Body: Schema.String }),
    Schema.Struct({ Checklist: ChecklistChange }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "IssueChange" });

type HomeCommand =
  | "Sync"
  | { readonly Clone: { readonly project: ProjectId } }
  | { readonly CreateIssue: { readonly edit: EditId; readonly issue: NewIssue } }
  | {
      readonly ChangeIssue: {
        readonly change: IssueChange;
        readonly edit: EditId;
        readonly issue: IssueId;
      };
    }
  | { readonly Canvas: { readonly issue: IssueId; readonly name: CanvasName } };

const HomeCommand = Schema.Union(
  [
    Schema.Literal("Sync"),
    Schema.Struct({ Clone: Schema.Struct({ project: ProjectId }) }),
    Schema.Struct({ CreateIssue: Schema.Struct({ edit: EditId, issue: NewIssue }) }),
    Schema.Struct({
      ChangeIssue: Schema.Struct({ change: IssueChange, edit: EditId, issue: IssueId }),
    }),
    Schema.Struct({ Canvas: Schema.Struct({ issue: IssueId, name: CanvasName }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "HomeCommand" });

type PushSubscription = {
  readonly auth: string;
  readonly endpoint: string;
  readonly p256dh: string;
};

const PushSubscription = Schema.Struct({
  auth: Schema.String,
  endpoint: Schema.String,
  p256dh: Schema.String,
}).annotate({ identifier: "PushSubscription" });

type PushCommand = "PublicKey" | { readonly Register: PushSubscription };

const PushCommand = Schema.Union(
  [Schema.Literal("PublicKey"), Schema.Struct({ Register: PushSubscription })],
  { mode: "oneOf" },
).annotate({ identifier: "PushCommand" });

type PairRequest = { readonly device: globalThis.ReadonlyArray<number>; readonly name: string };

const PairRequest = Schema.Struct({
  device: Schema.Array(
    Schema.Number.annotate({ format: "uint8" })
      .check(Schema.isInt().annotate({ expected: "an integer" }))
      .check(
        Schema.isGreaterThanOrEqualTo(0).annotate({
          expected: "a value greater than or equal to 0",
        }),
      )
      .check(
        Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
      ),
  )
    .check(Schema.isMinLength(32).annotate({ expected: "a value with a length of at least 32" }))
    .check(Schema.isMaxLength(32).annotate({ expected: "a value with a length of at most 32" })),
  name: Schema.String,
}).annotate({ identifier: "PairRequest" });

type EnrollmentToken = globalThis.ReadonlyArray<number>;

const EnrollmentToken = Schema.Array(
  Schema.Number.annotate({ format: "uint8" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    )
    .check(
      Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
    ),
)
  .check(Schema.isMinLength(16).annotate({ expected: "a value with a length of at least 16" }))
  .check(
    Schema.isMaxLength(16).annotate({
      expected: "a value with a length of at most 16",
      identifier: "EnrollmentToken",
    }),
  );

type FleetCommand =
  | "Status"
  | "Invite"
  | { readonly Revoke: globalThis.ReadonlyArray<number> }
  | { readonly Approve: PairRequest }
  | {
      readonly Admit: {
        readonly name: string;
        readonly node: globalThis.ReadonlyArray<number>;
        readonly relay: string;
        readonly token: EnrollmentToken;
      };
    };

const FleetCommand = Schema.Union(
  [
    Schema.Literals(["Status", "Invite"]),
    Schema.Struct({
      Revoke: Schema.Array(
        Schema.Number.annotate({ format: "uint8" })
          .check(Schema.isInt().annotate({ expected: "an integer" }))
          .check(
            Schema.isGreaterThanOrEqualTo(0).annotate({
              expected: "a value greater than or equal to 0",
            }),
          )
          .check(
            Schema.isLessThanOrEqualTo(255).annotate({
              expected: "a value less than or equal to 255",
            }),
          ),
      )
        .check(
          Schema.isMinLength(32).annotate({ expected: "a value with a length of at least 32" }),
        )
        .check(
          Schema.isMaxLength(32).annotate({ expected: "a value with a length of at most 32" }),
        ),
    }),
    Schema.Struct({ Approve: PairRequest }),
    Schema.Struct({
      Admit: Schema.Struct({
        name: Schema.String,
        node: Schema.Array(
          Schema.Number.annotate({ format: "uint8" })
            .check(Schema.isInt().annotate({ expected: "an integer" }))
            .check(
              Schema.isGreaterThanOrEqualTo(0).annotate({
                expected: "a value greater than or equal to 0",
              }),
            )
            .check(
              Schema.isLessThanOrEqualTo(255).annotate({
                expected: "a value less than or equal to 255",
              }),
            ),
        )
          .check(
            Schema.isMinLength(32).annotate({ expected: "a value with a length of at least 32" }),
          )
          .check(
            Schema.isMaxLength(32).annotate({ expected: "a value with a length of at most 32" }),
          ),
        relay: Schema.String,
        token: EnrollmentToken,
      }),
    }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "FleetCommand" });

type Command =
  | { readonly Session: SessionCommand }
  | { readonly Instance: InstanceCommand }
  | { readonly SignIn: SignInCommand }
  | { readonly Home: HomeCommand }
  | { readonly Push: PushCommand }
  | { readonly Fleet: FleetCommand };

const Command = Schema.Union(
  [
    Schema.Struct({ Session: SessionCommand }),
    Schema.Struct({ Instance: InstanceCommand }),
    Schema.Struct({ SignIn: SignInCommand }),
    Schema.Struct({ Home: HomeCommand }),
    Schema.Struct({ Push: PushCommand }),
    Schema.Struct({ Fleet: FleetCommand }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Command" });

type CommandId = string;

const CommandId = Schema.String.annotate({ format: "uuid", identifier: "CommandId" });

type HomeRefusal =
  | "NoHome"
  | "UnknownProject"
  | "NotCheckedOut"
  | "NoRemote"
  | "UnknownIssue"
  | "UnknownCanvas"
  | { readonly IssueNotSaved: { readonly reason: string } }
  | { readonly IssueRefused: { readonly reason: string } }
  | { readonly CanvasUnavailable: { readonly reason: string } };

const HomeRefusal = Schema.Union(
  [
    Schema.Literals([
      "NoHome",
      "UnknownProject",
      "NotCheckedOut",
      "NoRemote",
      "UnknownIssue",
      "UnknownCanvas",
    ]),
    Schema.Struct({ IssueNotSaved: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({ IssueRefused: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({ CanvasUnavailable: Schema.Struct({ reason: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "HomeRefusal" });

type SessionRefusal =
  | "NoSuchDirectory"
  | "UnknownSession"
  | "SessionClosed"
  | "TurnInProgress"
  | "NoTurnRunning"
  | "UnknownRequest"
  | "NotGitRepository"
  | "SnapshotUnavailable"
  | "UnknownInstance"
  | "SessionRemoved"
  | "NotOnIssue"
  | "AlreadyFinished"
  | { readonly InstanceUnavailable: { readonly reason: string } }
  | { readonly NotOffered: { readonly reason: string } }
  | { readonly AlreadyResolved: { readonly by: ClientName } }
  | { readonly WrongReply: { readonly reason: string } }
  | { readonly WorkspaceFailure: { readonly reason: string } }
  | { readonly Home: HomeRefusal }
  | { readonly IssueTaken: { readonly session: SessionId } };

const SessionRefusal = Schema.Union(
  [
    Schema.Literals([
      "NoSuchDirectory",
      "UnknownSession",
      "SessionClosed",
      "TurnInProgress",
      "NoTurnRunning",
      "UnknownRequest",
      "NotGitRepository",
      "SnapshotUnavailable",
      "UnknownInstance",
      "SessionRemoved",
      "NotOnIssue",
      "AlreadyFinished",
    ]),
    Schema.Struct({ InstanceUnavailable: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({ NotOffered: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({ AlreadyResolved: Schema.Struct({ by: ClientName }) }),
    Schema.Struct({ WrongReply: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({ WorkspaceFailure: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({ Home: HomeRefusal }),
    Schema.Struct({ IssueTaken: Schema.Struct({ session: SessionId }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "SessionRefusal" });

type DiffExtent = "Complete" | "Truncated";

const DiffExtent = Schema.Literals(["Complete", "Truncated"]).annotate({
  identifier: "DiffExtent",
});

type DiffChange = "Write" | "Edit" | "Delete";

const DiffChange = Schema.Literals(["Write", "Edit", "Delete"]).annotate({
  identifier: "DiffChange",
});

type FilePatch =
  | "Binary"
  | "TooLarge"
  | { readonly Text: string }
  | { readonly Unsupported: { readonly reason: string } };

const FilePatch = Schema.Union(
  [
    Schema.Literals(["Binary", "TooLarge"]),
    Schema.Struct({ Text: Schema.String }),
    Schema.Struct({ Unsupported: Schema.Struct({ reason: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "FilePatch" });

type SnapshotFile = {
  readonly change: DiffChange;
  readonly patch: FilePatch;
  readonly path: string;
};

const SnapshotFile = Schema.Struct({
  change: DiffChange,
  patch: FilePatch,
  path: Schema.String,
}).annotate({ identifier: "SnapshotFile" });

type SnapshotDiff = {
  readonly extent: DiffExtent;
  readonly files: globalThis.ReadonlyArray<SnapshotFile>;
  readonly snapshot: HostSeq;
};

const SnapshotDiff = Schema.Struct({
  extent: DiffExtent,
  files: Schema.Array(SnapshotFile),
  snapshot: HostSeq,
}).annotate({ identifier: "SnapshotDiff" });

type SessionResult =
  | "Accepted"
  | { readonly Started: { readonly session: SessionId } }
  | { readonly Refused: SessionRefusal }
  | { readonly Diff: SnapshotDiff };

const SessionResult = Schema.Union(
  [
    Schema.Literal("Accepted"),
    Schema.Struct({ Started: Schema.Struct({ session: SessionId }) }),
    Schema.Struct({ Refused: SessionRefusal }),
    Schema.Struct({ Diff: SnapshotDiff }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "SessionResult" });

type InstanceRefusal =
  | "UnknownInstance"
  | "FleetUnavailable"
  | { readonly CredentialNotSaved: { readonly reason: string } }
  | { readonly InvalidCredential: { readonly reason: string } }
  | { readonly InstanceInUse: { readonly sessions: number } };

const InstanceRefusal = Schema.Union(
  [
    Schema.Literals(["UnknownInstance", "FleetUnavailable"]),
    Schema.Struct({ CredentialNotSaved: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({ InvalidCredential: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({
      InstanceInUse: Schema.Struct({
        sessions: Schema.Number.annotate({ format: "uint64" })
          .check(Schema.isInt().annotate({ expected: "an integer" }))
          .check(
            Schema.isGreaterThanOrEqualTo(0).annotate({
              expected: "a value greater than or equal to 0",
            }),
          ),
      }),
    }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "InstanceRefusal" });

type InstanceResult = "Accepted" | { readonly Refused: InstanceRefusal };

const InstanceResult = Schema.Union(
  [Schema.Literal("Accepted"), Schema.Struct({ Refused: InstanceRefusal })],
  { mode: "oneOf" },
).annotate({ identifier: "InstanceResult" });

type SignInRefusal =
  | "UnknownInstance"
  | "SignInRunning"
  | "NoCodeAsked"
  | { readonly InstanceUnavailable: { readonly reason: string } }
  | { readonly InvalidCredential: { readonly reason: string } }
  | { readonly CredentialNotSaved: { readonly reason: string } }
  | { readonly InstanceInUse: { readonly sessions: number } };

const SignInRefusal = Schema.Union(
  [
    Schema.Literals(["UnknownInstance", "SignInRunning", "NoCodeAsked"]),
    Schema.Struct({ InstanceUnavailable: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({ InvalidCredential: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({ CredentialNotSaved: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({
      InstanceInUse: Schema.Struct({
        sessions: Schema.Number.annotate({ format: "uint64" })
          .check(Schema.isInt().annotate({ expected: "an integer" }))
          .check(
            Schema.isGreaterThanOrEqualTo(0).annotate({
              expected: "a value greater than or equal to 0",
            }),
          ),
      }),
    }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "SignInRefusal" });

type SignInResult = "Accepted" | { readonly Refused: SignInRefusal };

const SignInResult = Schema.Union(
  [Schema.Literal("Accepted"), Schema.Struct({ Refused: SignInRefusal })],
  { mode: "oneOf" },
).annotate({ identifier: "SignInResult" });

type HomeResult =
  | "Accepted"
  | {
      readonly IssueWritten: {
        readonly issue: IssueId;
        readonly warnings: globalThis.ReadonlyArray<string>;
      };
    }
  | { readonly Canvas: CanvasFile }
  | { readonly Refused: HomeRefusal };

const HomeResult = Schema.Union(
  [
    Schema.Literal("Accepted"),
    Schema.Struct({
      IssueWritten: Schema.Struct({ issue: IssueId, warnings: Schema.Array(Schema.String) }),
    }),
    Schema.Struct({ Canvas: CanvasFile }),
    Schema.Struct({ Refused: HomeRefusal }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "HomeResult" });

type PushResult = "Accepted" | "Unavailable" | { readonly PublicKey: string };

const PushResult = Schema.Union(
  [Schema.Literals(["Accepted", "Unavailable"]), Schema.Struct({ PublicKey: Schema.String })],
  { mode: "oneOf" },
).annotate({ identifier: "PushResult" });

type Signed = {
  readonly payload: globalThis.ReadonlyArray<number>;
  readonly signature: globalThis.ReadonlyArray<number>;
};

const Signed = Schema.Struct({
  payload: Schema.Array(
    Schema.Number.annotate({ format: "uint8" })
      .check(Schema.isInt().annotate({ expected: "an integer" }))
      .check(
        Schema.isGreaterThanOrEqualTo(0).annotate({
          expected: "a value greater than or equal to 0",
        }),
      )
      .check(
        Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
      ),
  ),
  signature: Schema.Array(
    Schema.Number.annotate({ format: "uint8" })
      .check(Schema.isInt().annotate({ expected: "an integer" }))
      .check(
        Schema.isGreaterThanOrEqualTo(0).annotate({
          expected: "a value greater than or equal to 0",
        }),
      )
      .check(
        Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
      ),
  ),
}).annotate({ identifier: "Signed" });

type InviteSecret = globalThis.ReadonlyArray<number>;

const InviteSecret = Schema.Array(
  Schema.Number.annotate({ format: "uint8" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    )
    .check(
      Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
    ),
)
  .check(Schema.isMinLength(32).annotate({ expected: "a value with a length of at least 32" }))
  .check(
    Schema.isMaxLength(32).annotate({
      expected: "a value with a length of at most 32",
      identifier: "InviteSecret",
    }),
  );

type Invite = {
  readonly fleet: globalThis.ReadonlyArray<number>;
  readonly relay: string;
  readonly root: globalThis.ReadonlyArray<number>;
  readonly secret: InviteSecret;
};

const Invite = Schema.Struct({
  fleet: Schema.Array(
    Schema.Number.annotate({ format: "uint8" })
      .check(Schema.isInt().annotate({ expected: "an integer" }))
      .check(
        Schema.isGreaterThanOrEqualTo(0).annotate({
          expected: "a value greater than or equal to 0",
        }),
      )
      .check(
        Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
      ),
  )
    .check(Schema.isMinLength(32).annotate({ expected: "a value with a length of at least 32" }))
    .check(Schema.isMaxLength(32).annotate({ expected: "a value with a length of at most 32" })),
  relay: Schema.String,
  root: Schema.Array(
    Schema.Number.annotate({ format: "uint8" })
      .check(Schema.isInt().annotate({ expected: "an integer" }))
      .check(
        Schema.isGreaterThanOrEqualTo(0).annotate({
          expected: "a value greater than or equal to 0",
        }),
      )
      .check(
        Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
      ),
  )
    .check(Schema.isMinLength(32).annotate({ expected: "a value with a length of at least 32" }))
    .check(Schema.isMaxLength(32).annotate({ expected: "a value with a length of at most 32" })),
  secret: InviteSecret,
}).annotate({ identifier: "Invite" });

type HostEntry = {
  readonly added_at: number;
  readonly name: string;
  readonly node: globalThis.ReadonlyArray<number>;
  readonly relay: string;
};

const HostEntry = Schema.Struct({
  added_at: Schema.Number.annotate({ format: "uint64" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
  name: Schema.String,
  node: Schema.Array(
    Schema.Number.annotate({ format: "uint8" })
      .check(Schema.isInt().annotate({ expected: "an integer" }))
      .check(
        Schema.isGreaterThanOrEqualTo(0).annotate({
          expected: "a value greater than or equal to 0",
        }),
      )
      .check(
        Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
      ),
  )
    .check(Schema.isMinLength(32).annotate({ expected: "a value with a length of at least 32" }))
    .check(Schema.isMaxLength(32).annotate({ expected: "a value with a length of at most 32" })),
  relay: Schema.String,
}).annotate({ identifier: "HostEntry" });

type Pairing = { readonly certificate: Signed; readonly root: HostEntry; readonly signed: Signed };

const Pairing = Schema.Struct({ certificate: Signed, root: HostEntry, signed: Signed }).annotate({
  identifier: "Pairing",
});

type FleetResult =
  | "Accepted"
  | "Unavailable"
  | { readonly Status: Signed }
  | { readonly Invite: Invite }
  | { readonly Pairing: Pairing };

const FleetResult = Schema.Union(
  [
    Schema.Literals(["Accepted", "Unavailable"]),
    Schema.Struct({ Status: Signed }),
    Schema.Struct({ Invite: Invite }),
    Schema.Struct({ Pairing: Pairing }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "FleetResult" });

type CommandResult =
  | { readonly Session: SessionResult }
  | { readonly Instance: InstanceResult }
  | { readonly SignIn: SignInResult }
  | { readonly Home: HomeResult }
  | { readonly Push: PushResult }
  | { readonly Fleet: FleetResult };

const CommandResult = Schema.Union(
  [
    Schema.Struct({ Session: SessionResult }),
    Schema.Struct({ Instance: InstanceResult }),
    Schema.Struct({ SignIn: SignInResult }),
    Schema.Struct({ Home: HomeResult }),
    Schema.Struct({ Push: PushResult }),
    Schema.Struct({ Fleet: FleetResult }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "CommandResult" });

type Coverage = "Complete" | { readonly Incomplete: { readonly reason: string } };

const Coverage = Schema.Union(
  [
    Schema.Literal("Complete"),
    Schema.Struct({ Incomplete: Schema.Struct({ reason: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Coverage" });

type Completeness = {
  readonly fields: Coverage;
  readonly notes: Coverage;
  readonly relations: { readonly [x: string]: Coverage };
};

const Completeness = Schema.Struct({
  fields: Coverage,
  notes: Coverage,
  relations: Schema.Record(Schema.String, Coverage),
}).annotate({ identifier: "Completeness" });

type DeviceHello = {
  readonly certificate: Signed;
  readonly proof: globalThis.ReadonlyArray<number>;
};

const DeviceHello = Schema.Struct({
  certificate: Signed,
  proof: Schema.Array(
    Schema.Number.annotate({ format: "uint8" })
      .check(Schema.isInt().annotate({ expected: "an integer" }))
      .check(
        Schema.isGreaterThanOrEqualTo(0).annotate({
          expected: "a value greater than or equal to 0",
        }),
      )
      .check(
        Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
      ),
  ),
}).annotate({ identifier: "DeviceHello" });

type LogEvent =
  | "Note"
  | { readonly Stage: { readonly from: string; readonly to: string } }
  | { readonly Status: { readonly from: string; readonly to: string } };

const LogEvent = Schema.Union(
  [
    Schema.Literal("Note"),
    Schema.Struct({ Stage: Schema.Struct({ from: Schema.String, to: Schema.String }) }),
    Schema.Struct({ Status: Schema.Struct({ from: Schema.String, to: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "LogEvent" });

type Entry = {
  readonly at: string | null;
  readonly by: string | null;
  readonly event: LogEvent;
  readonly text: string;
};

const Entry = Schema.Struct({
  at: Schema.Union([Schema.String, Schema.Null]),
  by: Schema.Union([Schema.String, Schema.Null]),
  event: LogEvent,
  text: Schema.String,
}).annotate({ identifier: "Entry" });

type Epoch = string;

const Epoch = Schema.String.annotate({ identifier: "Epoch" });

type ToolCallId = string;

const ToolCallId = Schema.String.annotate({ identifier: "ToolCallId" });

type FileChange = { readonly change: Change2; readonly path: string };

const FileChange = Schema.Struct({ change: Change2, path: Schema.String }).annotate({
  identifier: "FileChange",
});

type ToolKind =
  | { readonly Command: { readonly command: string } }
  | { readonly FileChange: { readonly files: globalThis.ReadonlyArray<FileChange> } }
  | { readonly Other: { readonly detail: string; readonly name: string } };

const ToolKind = Schema.Union(
  [
    Schema.Struct({ Command: Schema.Struct({ command: Schema.String }) }),
    Schema.Struct({ FileChange: Schema.Struct({ files: Schema.Array(FileChange) }) }),
    Schema.Struct({ Other: Schema.Struct({ detail: Schema.String, name: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "ToolKind" });

type ToolOutcome = "Succeeded" | "Failed" | "Declined";

const ToolOutcome = Schema.Literals(["Succeeded", "Failed", "Declined"]).annotate({
  identifier: "ToolOutcome",
});

type ToolPhase =
  | { readonly Started: { readonly tool: ToolKind } }
  | { readonly Finished: { readonly outcome: ToolOutcome } };

const ToolPhase = Schema.Union(
  [
    Schema.Struct({ Started: Schema.Struct({ tool: ToolKind }) }),
    Schema.Struct({ Finished: Schema.Struct({ outcome: ToolOutcome }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "ToolPhase" });

type Output =
  | { readonly AssistantDelta: { readonly text: string } }
  | { readonly AssistantMessage: { readonly text: string } }
  | { readonly ReasoningDelta: { readonly text: string } }
  | { readonly Reasoning: { readonly text: string } }
  | { readonly ToolCall: { readonly call: ToolCallId; readonly phase: ToolPhase } }
  | { readonly Error: { readonly message: string } };

const Output = Schema.Union(
  [
    Schema.Struct({ AssistantDelta: Schema.Struct({ text: Schema.String }) }),
    Schema.Struct({ AssistantMessage: Schema.Struct({ text: Schema.String }) }),
    Schema.Struct({ ReasoningDelta: Schema.Struct({ text: Schema.String }) }),
    Schema.Struct({ Reasoning: Schema.Struct({ text: Schema.String }) }),
    Schema.Struct({ ToolCall: Schema.Struct({ call: ToolCallId, phase: ToolPhase }) }),
    Schema.Struct({ Error: Schema.Struct({ message: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Output" });

type QuestionOption = { readonly description: string; readonly label: string };

const QuestionOption = Schema.Struct({ description: Schema.String, label: Schema.String }).annotate(
  { identifier: "QuestionOption" },
);

type Selection = "One" | "Many";

const Selection = Schema.Literals(["One", "Many"]).annotate({ identifier: "Selection" });

type Question = {
  readonly header: string;
  readonly options: globalThis.ReadonlyArray<QuestionOption>;
  readonly selection: Selection;
  readonly text: string;
};

const Question = Schema.Struct({
  header: Schema.String,
  options: Schema.Array(QuestionOption),
  selection: Selection,
  text: Schema.String,
}).annotate({ identifier: "Question" });

type Resolution =
  | "Withdrawn"
  | { readonly Decided: { readonly by: ClientName; readonly decision: Decision } }
  | {
      readonly Answered: {
        readonly answers: globalThis.ReadonlyArray<Answer>;
        readonly by: ClientName;
      };
    };

const Resolution = Schema.Union(
  [
    Schema.Literal("Withdrawn"),
    Schema.Struct({ Decided: Schema.Struct({ by: ClientName, decision: Decision }) }),
    Schema.Struct({ Answered: Schema.Struct({ answers: Schema.Array(Answer), by: ClientName }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Resolution" });

type TurnOutcome = "Completed" | "Interrupted" | { readonly Failed: { readonly message: string } };

const TurnOutcome = Schema.Union(
  [
    Schema.Literals(["Completed", "Interrupted"]),
    Schema.Struct({ Failed: Schema.Struct({ message: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "TurnOutcome" });

type WorkingCopy = string;

const WorkingCopy = Schema.String.annotate({ identifier: "WorkingCopy" });

type Repository = string;

const Repository = Schema.String.annotate({ identifier: "Repository" });

type Worktree = string;

const Worktree = Schema.String.annotate({ identifier: "Worktree" });

type Workspace = {
  readonly branch: string;
  readonly directory: SessionDirectory;
  readonly original: WorkingCopy;
  readonly repository: Repository;
  readonly worktree: Worktree;
};

const Workspace = Schema.Struct({
  branch: Schema.String,
  directory: SessionDirectory,
  original: WorkingCopy,
  repository: Repository,
  worktree: Worktree,
}).annotate({ identifier: "Workspace" });

type Snapshot = { readonly commit: string; readonly turn_start: HostSeq | null };

const Snapshot = Schema.Struct({
  commit: Schema.String,
  turn_start: Schema.Union([HostSeq, Schema.Null]),
}).annotate({ identifier: "Snapshot" });

type SubagentId = string;

const SubagentId = Schema.String.annotate({ identifier: "SubagentId" });

type SubagentActivity =
  | { readonly Message: { readonly text: string } }
  | { readonly Reasoning: { readonly text: string } }
  | { readonly ToolCall: { readonly call: ToolCallId; readonly phase: ToolPhase } };

const SubagentActivity = Schema.Union(
  [
    Schema.Struct({ Message: Schema.Struct({ text: Schema.String }) }),
    Schema.Struct({ Reasoning: Schema.Struct({ text: Schema.String }) }),
    Schema.Struct({ ToolCall: Schema.Struct({ call: ToolCallId, phase: ToolPhase }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "SubagentActivity" });

type SubagentOutcome = "Finished" | "Failed" | "Stopped";

const SubagentOutcome = Schema.Literals(["Finished", "Failed", "Stopped"]).annotate({
  identifier: "SubagentOutcome",
});

type SubagentEvent =
  | { readonly Started: { readonly call: ToolCallId; readonly description: string } }
  | { readonly Activity: SubagentActivity }
  | { readonly Ended: { readonly outcome: SubagentOutcome; readonly summary: string | null } }
  | { readonly Kind: { readonly kind: string } }
  | { readonly ChecklistChanged: { readonly checklist: globalThis.ReadonlyArray<ChecklistEntry> } }
  | {
      readonly RequestOpened: {
        readonly call: ToolCallId;
        readonly for_session: string | null;
        readonly reason: string | null;
        readonly request: RequestId;
        readonly tool: ToolKind;
      };
    }
  | {
      readonly QuestionAsked: {
        readonly questions: globalThis.ReadonlyArray<Question>;
        readonly request: RequestId;
      };
    };

const SubagentEvent = Schema.Union(
  [
    Schema.Struct({ Started: Schema.Struct({ call: ToolCallId, description: Schema.String }) }),
    Schema.Struct({ Activity: SubagentActivity }),
    Schema.Struct({
      Ended: Schema.Struct({
        outcome: SubagentOutcome,
        summary: Schema.Union([Schema.String, Schema.Null]),
      }),
    }),
    Schema.Struct({ Kind: Schema.Struct({ kind: Schema.String }) }),
    Schema.Struct({ ChecklistChanged: Schema.Struct({ checklist: Schema.Array(ChecklistEntry) }) }),
    Schema.Struct({
      RequestOpened: Schema.Struct({
        call: ToolCallId,
        for_session: Schema.Union([Schema.String, Schema.Null]),
        reason: Schema.Union([Schema.String, Schema.Null]),
        request: RequestId,
        tool: ToolKind,
      }),
    }),
    Schema.Struct({
      QuestionAsked: Schema.Struct({ questions: Schema.Array(Question), request: RequestId }),
    }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "SubagentEvent" });

type Kept =
  | "Local"
  | { readonly Pushed: { readonly remote: string } }
  | { readonly PushFailed: { readonly reason: string } };

const Kept = Schema.Union(
  [
    Schema.Literal("Local"),
    Schema.Struct({ Pushed: Schema.Struct({ remote: Schema.String }) }),
    Schema.Struct({ PushFailed: Schema.Struct({ reason: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Kept" });

type Finished =
  | { readonly Landed: { readonly commits: number; readonly kept: Kept } }
  | { readonly Failed: { readonly reason: string } };

const Finished = Schema.Union(
  [
    Schema.Struct({
      Landed: Schema.Struct({
        commits: Schema.Number.annotate({ format: "uint64" })
          .check(Schema.isInt().annotate({ expected: "an integer" }))
          .check(
            Schema.isGreaterThanOrEqualTo(0).annotate({
              expected: "a value greater than or equal to 0",
            }),
          ),
        kept: Kept,
      }),
    }),
    Schema.Struct({ Failed: Schema.Struct({ reason: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Finished" });

type EventKind =
  | "SessionReady"
  | "FinishStarted"
  | "FinishInterrupted"
  | {
      readonly SessionCreated: {
        readonly by: ClientName;
        readonly directory: SessionDirectory;
        readonly effort: Effort | null;
        readonly instance: InstanceId;
        readonly model: ModelId | null;
      };
    }
  | { readonly SessionClosed: { readonly reason: CloseReason } }
  | { readonly UserMessage: { readonly by: ClientName; readonly text: string } }
  | { readonly Agent: Output }
  | {
      readonly RequestOpened: {
        readonly call: ToolCallId;
        readonly for_session: string | null;
        readonly reason: string | null;
        readonly request: RequestId;
        readonly tool: ToolKind;
      };
    }
  | {
      readonly QuestionAsked: {
        readonly questions: globalThis.ReadonlyArray<Question>;
        readonly request: RequestId;
      };
    }
  | { readonly RequestResolved: { readonly request: RequestId; readonly resolution: Resolution } }
  | { readonly TurnCompleted: { readonly outcome: TurnOutcome } }
  | { readonly Error: { readonly message: string } }
  | {
      readonly ModelChanged: {
        readonly effort: Effort | null;
        readonly model: ModelId | null;
        readonly reason: string;
      };
    }
  | { readonly WorkspaceCreated: { readonly workspace: Workspace } }
  | { readonly SnapshotRecorded: { readonly snapshot: Snapshot } }
  | { readonly NativeTurnStarted: { readonly turn_start: HostSeq | null } }
  | { readonly ChecklistChanged: { readonly checklist: globalThis.ReadonlyArray<ChecklistEntry> } }
  | { readonly EffortChanged: { readonly effort: Effort } }
  | { readonly Subagent: { readonly agent: SubagentId; readonly event: SubagentEvent } }
  | { readonly SessionRemoved: { readonly kept: Kept | null } }
  | { readonly OnIssue: { readonly issue: IssueId } }
  | { readonly SessionFinished: { readonly finished: Finished } };

const EventKind = Schema.Union(
  [
    Schema.Literals(["SessionReady", "FinishStarted", "FinishInterrupted"]),
    Schema.Struct({
      SessionCreated: Schema.Struct({
        by: ClientName,
        directory: SessionDirectory,
        effort: Schema.Union([Effort, Schema.Null]),
        instance: InstanceId,
        model: Schema.Union([ModelId, Schema.Null]),
      }),
    }),
    Schema.Struct({ SessionClosed: Schema.Struct({ reason: CloseReason }) }),
    Schema.Struct({ UserMessage: Schema.Struct({ by: ClientName, text: Schema.String }) }),
    Schema.Struct({ Agent: Output }),
    Schema.Struct({
      RequestOpened: Schema.Struct({
        call: ToolCallId,
        for_session: Schema.Union([Schema.String, Schema.Null]),
        reason: Schema.Union([Schema.String, Schema.Null]),
        request: RequestId,
        tool: ToolKind,
      }),
    }),
    Schema.Struct({
      QuestionAsked: Schema.Struct({ questions: Schema.Array(Question), request: RequestId }),
    }),
    Schema.Struct({
      RequestResolved: Schema.Struct({ request: RequestId, resolution: Resolution }),
    }),
    Schema.Struct({ TurnCompleted: Schema.Struct({ outcome: TurnOutcome }) }),
    Schema.Struct({ Error: Schema.Struct({ message: Schema.String }) }),
    Schema.Struct({
      ModelChanged: Schema.Struct({
        effort: Schema.Union([Effort, Schema.Null]),
        model: Schema.Union([ModelId, Schema.Null]),
        reason: Schema.String,
      }),
    }),
    Schema.Struct({ WorkspaceCreated: Schema.Struct({ workspace: Workspace }) }),
    Schema.Struct({ SnapshotRecorded: Schema.Struct({ snapshot: Snapshot }) }),
    Schema.Struct({
      NativeTurnStarted: Schema.Struct({ turn_start: Schema.Union([HostSeq, Schema.Null]) }),
    }),
    Schema.Struct({ ChecklistChanged: Schema.Struct({ checklist: Schema.Array(ChecklistEntry) }) }),
    Schema.Struct({ EffortChanged: Schema.Struct({ effort: Effort }) }),
    Schema.Struct({ Subagent: Schema.Struct({ agent: SubagentId, event: SubagentEvent }) }),
    Schema.Struct({ SessionRemoved: Schema.Struct({ kept: Schema.Union([Kept, Schema.Null]) }) }),
    Schema.Struct({ OnIssue: Schema.Struct({ issue: IssueId }) }),
    Schema.Struct({ SessionFinished: Schema.Struct({ finished: Finished }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "EventKind" });

type Event = { readonly kind: EventKind; readonly seq: HostSeq; readonly session: SessionId };

const Event = Schema.Struct({ kind: EventKind, seq: HostSeq, session: SessionId }).annotate({
  identifier: "Event",
});

type SignInStep =
  | "Starting"
  | "Finishing"
  | "SignedIn"
  | "Cancelled"
  | {
      readonly Open: {
        readonly code: string | null;
        readonly paste: boolean;
        readonly url: string;
      };
    }
  | { readonly Failed: { readonly reason: string } };

const SignInStep = Schema.Union(
  [
    Schema.Literals(["Starting", "Finishing", "SignedIn", "Cancelled"]),
    Schema.Struct({
      Open: Schema.Struct({
        code: Schema.Union([Schema.String, Schema.Null]),
        paste: Schema.Boolean,
        url: Schema.String,
      }),
    }),
    Schema.Struct({ Failed: Schema.Struct({ reason: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "SignInStep" });

type SignIn = { readonly instance: InstanceId; readonly step: SignInStep };

const SignIn = Schema.Struct({ instance: InstanceId, step: SignInStep }).annotate({
  identifier: "SignIn",
});

type FieldRole = "labels" | "branch" | "created";

const FieldRole = Schema.Literals(["labels", "branch", "created"]).annotate({
  identifier: "FieldRole",
});

type FieldType =
  | "Text"
  | "Texts"
  | "Time"
  | "Bool"
  | { readonly Enum: globalThis.ReadonlyArray<string> };

const FieldType = Schema.Union(
  [
    Schema.Literals(["Text", "Texts", "Time", "Bool"]),
    Schema.Struct({ Enum: Schema.Array(Schema.String) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "FieldType" });

type FlowField = {
  readonly default: Datum | null;
  readonly name: string;
  readonly role: FieldRole | null;
  readonly value: FieldType;
};

const FlowField = Schema.Struct({
  default: Schema.Union([Datum, Schema.Null]),
  name: Schema.String,
  role: Schema.Union([FieldRole, Schema.Null]),
  value: FieldType,
}).annotate({ identifier: "FlowField" });

type Kind = {
  readonly colour: string | null;
  readonly fields: globalThis.ReadonlyArray<FlowField>;
  readonly icon: string | null;
  readonly label: string;
  readonly name: string;
  readonly order: globalThis.ReadonlyArray<string>;
  readonly sections: globalThis.ReadonlyArray<string>;
};

const Kind = Schema.Struct({
  colour: Schema.Union([Schema.String, Schema.Null]),
  fields: Schema.Array(FlowField),
  icon: Schema.Union([Schema.String, Schema.Null]),
  label: Schema.String,
  name: Schema.String,
  order: Schema.Array(Schema.String),
  sections: Schema.Array(Schema.String),
}).annotate({ identifier: "Kind" });

type Shape = "hierarchy" | "dependency";

const Shape = Schema.Literals(["hierarchy", "dependency"]).annotate({ identifier: "Shape" });

type Relation = {
  readonly cycles: boolean;
  readonly from: globalThis.ReadonlyArray<string>;
  readonly many: boolean;
  readonly name: string;
  readonly reverse: string;
  readonly shape: Shape;
  readonly to: globalThis.ReadonlyArray<string>;
};

const Relation = Schema.Struct({
  cycles: Schema.Boolean.annotate({ default: false }),
  from: Schema.Array(Schema.String).annotate({ default: [] }),
  many: Schema.Boolean.annotate({ default: false }),
  name: Schema.String,
  reverse: Schema.String,
  shape: Shape,
  to: Schema.Array(Schema.String).annotate({ default: [] }),
}).annotate({ identifier: "Relation" });

type Role = "idle" | "startable" | "active" | "review" | "terminal" | "dropped";

const Role = Schema.Literals([
  "idle",
  "startable",
  "active",
  "review",
  "terminal",
  "dropped",
]).annotate({ identifier: "Role" });

type Literal = { readonly Text: string } | { readonly Bool: boolean };

const Literal = Schema.Union(
  [Schema.Struct({ Text: Schema.String }), Schema.Struct({ Bool: Schema.Boolean })],
  { mode: "oneOf" },
).annotate({ identifier: "Literal" });

type Quantifier = "Any" | "All" | "None";

const Quantifier = Schema.Literals(["Any", "All", "None"]).annotate({ identifier: "Quantifier" });

type Predicate =
  | { readonly Kind: string }
  | { readonly Stage: string }
  | { readonly StatusRole: Role }
  | { readonly Field: { readonly name: string; readonly value: Literal } }
  | { readonly Has: string }
  | { readonly HasSection: string }
  | { readonly From: Predicate }
  | { readonly To: Predicate }
  | { readonly All: globalThis.ReadonlyArray<Predicate> }
  | { readonly Any: globalThis.ReadonlyArray<Predicate> }
  | { readonly Not: Predicate }
  | {
      readonly Across: {
        readonly condition: Predicate;
        readonly quantifier: Quantifier;
        readonly via: string;
      };
    };

const Predicate = Schema.Union(
  [
    Schema.Struct({ Kind: Schema.String }),
    Schema.Struct({ Stage: Schema.String }),
    Schema.Struct({ StatusRole: Role }),
    Schema.Struct({ Field: Schema.Struct({ name: Schema.String, value: Literal }) }),
    Schema.Struct({ Has: Schema.String }),
    Schema.Struct({ HasSection: Schema.String }),
    Schema.Struct({ From: Schema.suspend((): Schema.Codec<Predicate> => Predicate) }),
    Schema.Struct({ To: Schema.suspend((): Schema.Codec<Predicate> => Predicate) }),
    Schema.Struct({ All: Schema.Array(Schema.suspend((): Schema.Codec<Predicate> => Predicate)) }),
    Schema.Struct({ Any: Schema.Array(Schema.suspend((): Schema.Codec<Predicate> => Predicate)) }),
    Schema.Struct({ Not: Schema.suspend((): Schema.Codec<Predicate> => Predicate) }),
    Schema.Struct({
      Across: Schema.Struct({
        condition: Schema.suspend((): Schema.Codec<Predicate> => Predicate),
        quantifier: Quantifier,
        via: Schema.String,
      }),
    }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Predicate" });

type Severity = "warn" | "error";

const Severity = Schema.Literals(["warn", "error"]).annotate({ identifier: "Severity" });

type Rule = {
  readonly message: string;
  readonly require: Predicate;
  readonly severity: Severity;
  readonly when: Predicate;
};

const Rule = Schema.Struct({
  message: Schema.String,
  require: Predicate,
  severity: Severity,
  when: Predicate,
}).annotate({ identifier: "Rule" });

type Stage = {
  readonly name: string;
  readonly next: globalThis.ReadonlyArray<string>;
  readonly sign_off: boolean;
  readonly skills: globalThis.ReadonlyArray<string>;
};

const Stage = Schema.Struct({
  name: Schema.String,
  next: Schema.Array(Schema.String).annotate({ default: [] }),
  sign_off: Schema.Boolean.annotate({ default: false }),
  skills: Schema.Array(Schema.String).annotate({ default: [] }),
}).annotate({ identifier: "Stage" });

type Status = { readonly colour: string | null; readonly name: string; readonly role: Role };

const Status = Schema.Struct({
  colour: Schema.Union([Schema.String, Schema.Null]),
  name: Schema.String,
  role: Role,
}).annotate({ identifier: "Status" });

type Flow = {
  readonly default_kind: string;
  readonly fields: globalThis.ReadonlyArray<FlowField>;
  readonly kinds: globalThis.ReadonlyArray<Kind>;
  readonly relations: globalThis.ReadonlyArray<Relation>;
  readonly rules: globalThis.ReadonlyArray<Rule>;
  readonly stages: globalThis.ReadonlyArray<Stage>;
  readonly statuses: globalThis.ReadonlyArray<Status>;
};

const Flow = Schema.Struct({
  default_kind: Schema.String,
  fields: Schema.Array(FlowField),
  kinds: Schema.Array(Kind),
  relations: Schema.Array(Relation),
  rules: Schema.Array(Rule),
  stages: Schema.Array(Stage),
  statuses: Schema.Array(Status),
}).annotate({ identifier: "Flow" });

type Finding = {
  readonly message: string;
  readonly severity: Severity;
  readonly subject: string | null;
};

const Finding = Schema.Struct({
  message: Schema.String,
  severity: Severity,
  subject: Schema.Union([Schema.String, Schema.Null]),
}).annotate({ identifier: "Finding" });

type Locator = {
  readonly addresses: { readonly [x: string]: string };
  readonly reference: string;
  readonly repository: string;
  readonly url: string | null;
};

const Locator = Schema.Struct({
  addresses: Schema.Record(Schema.String, Schema.String),
  reference: Schema.String,
  repository: Schema.String,
  url: Schema.Union([Schema.String, Schema.Null]),
}).annotate({ identifier: "Locator" });

type Note = {
  readonly author: string;
  readonly created: number;
  readonly id: string;
  readonly text: string;
};

const Note = Schema.Struct({
  author: Schema.String,
  created: Schema.Number.annotate({ format: "uint64" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
  id: Schema.String,
  text: Schema.String,
}).annotate({ identifier: "Note" });

type Section = { readonly heading: string; readonly text: string };

const Section = Schema.Struct({ heading: Schema.String, text: Schema.String }).annotate({
  identifier: "Section",
});

type Issue = {
  readonly body: string;
  readonly branch: string;
  readonly canvases: globalThis.ReadonlyArray<CanvasEntry>;
  readonly checklist: globalThis.ReadonlyArray<ChecklistItem>;
  readonly created: number | null;
  readonly fields: { readonly [x: string]: Datum };
  readonly id: IssueId;
  readonly kind: string;
  readonly links: { readonly [x: string]: globalThis.ReadonlyArray<IssueId> };
  readonly log: globalThis.ReadonlyArray<Entry>;
  readonly notes: globalThis.ReadonlyArray<Note>;
  readonly project: ProjectId;
  readonly sections: globalThis.ReadonlyArray<Section>;
  readonly stage: string | null;
  readonly status: string;
  readonly title: string;
};

const Issue = Schema.Struct({
  body: Schema.String,
  branch: Schema.String,
  canvases: Schema.Array(CanvasEntry),
  checklist: Schema.Array(ChecklistItem),
  created: Schema.Union([
    Schema.Int.check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
    Schema.Null,
  ]).annotate({ format: "uint64" }),
  fields: Schema.Record(Schema.String, Datum),
  id: IssueId,
  kind: Schema.String,
  links: Schema.Record(Schema.String, Schema.Array(IssueId)),
  log: Schema.Array(Entry),
  notes: Schema.Array(Note),
  project: ProjectId,
  sections: Schema.Array(Section),
  stage: Schema.Union([Schema.String, Schema.Null]),
  status: Schema.String,
  title: Schema.String,
}).annotate({ identifier: "Issue" });

type Unreadable = {
  readonly body: string | null;
  readonly created: number | null;
  readonly id: IssueId;
  readonly project: ProjectId;
  readonly reason: string;
  readonly title: string | null;
};

const Unreadable = Schema.Struct({
  body: Schema.Union([Schema.String, Schema.Null]),
  created: Schema.Union([
    Schema.Int.check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
    Schema.Null,
  ]).annotate({ format: "uint64" }),
  id: IssueId,
  project: ProjectId,
  reason: Schema.String,
  title: Schema.Union([Schema.String, Schema.Null]),
}).annotate({ identifier: "Unreadable" });

type IssueState =
  | { readonly Valid: { readonly body: Body; readonly issue: Issue } }
  | { readonly Invalid: Unreadable }
  | { readonly Unavailable: Unreadable }
  | { readonly Deleted: Unreadable };

const IssueState = Schema.Union(
  [
    Schema.Struct({ Valid: Schema.Struct({ body: Body, issue: Issue }) }),
    Schema.Struct({ Invalid: Unreadable }),
    Schema.Struct({ Unavailable: Unreadable }),
    Schema.Struct({ Deleted: Unreadable }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "IssueState" });

type Observation = {
  readonly completeness: Completeness;
  readonly findings: globalThis.ReadonlyArray<Finding>;
  readonly flow_findings: globalThis.ReadonlyArray<Finding>;
  readonly locator: Locator | null;
  readonly observed_at: number | null;
  readonly revision: string | null;
  readonly state: IssueState;
};

const Observation = Schema.Struct({
  completeness: Completeness,
  findings: Schema.Array(Finding),
  flow_findings: Schema.Array(Finding),
  locator: Schema.Union([Locator, Schema.Null]),
  observed_at: Schema.Union([
    Schema.Int.check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
    Schema.Null,
  ]).annotate({ format: "uint64" }),
  revision: Schema.Union([Schema.String, Schema.Null]),
  state: IssueState,
}).annotate({ identifier: "Observation" });

type Project = {
  readonly checkouts: globalThis.ReadonlyArray<Checkout>;
  readonly cloning: Cloning;
  readonly id: ProjectId;
  readonly issues: globalThis.ReadonlyArray<Observation>;
  readonly name: string;
  readonly profiles: globalThis.ReadonlyArray<string>;
  readonly provides: globalThis.ReadonlyArray<string>;
  readonly remote: string | null;
  readonly title: string;
};

const Project = Schema.Struct({
  checkouts: Schema.Array(Checkout),
  cloning: Cloning,
  id: ProjectId,
  issues: Schema.Array(Observation),
  name: Schema.String,
  profiles: Schema.Array(Schema.String),
  provides: Schema.Array(Schema.String),
  remote: Schema.Union([Schema.String, Schema.Null]),
  title: Schema.String,
}).annotate({ identifier: "Project" });

type HomeSync =
  | "Syncing"
  | "Synced"
  | { readonly Behind: { readonly reason: string } }
  | { readonly Failed: { readonly reason: string } };

const HomeSync = Schema.Union(
  [
    Schema.Literals(["Syncing", "Synced"]),
    Schema.Struct({ Behind: Schema.Struct({ reason: Schema.String }) }),
    Schema.Struct({ Failed: Schema.Struct({ reason: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "HomeSync" });

type Home = {
  readonly flow: Flow | null;
  readonly projects: globalThis.ReadonlyArray<Project>;
  readonly remote: string;
  readonly sync: HomeSync;
  readonly synced: number | null;
  readonly unpushed: number;
};

const Home = Schema.Struct({
  flow: Schema.Union([Flow, Schema.Null]),
  projects: Schema.Array(Project),
  remote: Schema.String,
  sync: HomeSync,
  synced: Schema.Union([
    Schema.Int.check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
    Schema.Null,
  ]).annotate({ format: "uint64" }),
  unpushed: Schema.Number.annotate({ format: "uint64" })
    .check(Schema.isInt().annotate({ expected: "an integer" }))
    .check(
      Schema.isGreaterThanOrEqualTo(0).annotate({ expected: "a value greater than or equal to 0" }),
    ),
}).annotate({ identifier: "Home" });

type InstanceStatus = { readonly availability: Availability };

const InstanceStatus = Schema.Struct({ availability: Availability }).annotate({
  identifier: "InstanceStatus",
});

type Instances = globalThis.ReadonlyArray<readonly [InstanceId, InstanceStatus]>;

const Instances = Schema.Array(Schema.Tuple([InstanceId, InstanceStatus])).annotate({
  identifier: "Instances",
});

type Feed =
  | { readonly Event: Event }
  | { readonly SignIn: SignIn }
  | { readonly Home: Home }
  | { readonly Instances: Instances };

const Feed = Schema.Union(
  [
    Schema.Struct({ Event: Event }),
    Schema.Struct({ SignIn: SignIn }),
    Schema.Struct({ Home: Home }),
    Schema.Struct({ Instances: Instances }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Feed" });

type Hello = { readonly client: ClientName; readonly token: AuthToken };

const Hello = Schema.Struct({ client: ClientName, token: AuthToken }).annotate({
  identifier: "Hello",
});

type Welcome = {
  readonly catalog: Catalog;
  readonly epoch: Epoch;
  readonly head_seq: HostSeq;
  readonly home: Home | null;
  readonly host: string;
  readonly instances: Instances;
};

const Welcome = Schema.Struct({
  catalog: Catalog,
  epoch: Epoch,
  head_seq: HostSeq,
  home: Schema.Union([Home, Schema.Null]),
  host: Schema.String,
  instances: Instances,
}).annotate({ identifier: "Welcome" });

type HelloReply = { readonly Welcome: Welcome } | { readonly Refused: { readonly reason: string } };

const HelloReply = Schema.Union(
  [
    Schema.Struct({ Welcome: Welcome }),
    Schema.Struct({ Refused: Schema.Struct({ reason: Schema.String }) }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "HelloReply" });

type Subscribe = { readonly after_seq: HostSeq; readonly epoch: Epoch };

const Subscribe = Schema.Struct({ after_seq: HostSeq, epoch: Epoch }).annotate({
  identifier: "Subscribe",
});

type Open =
  | { readonly Hello: Hello }
  | { readonly Subscribe: Subscribe }
  | { readonly Command: { readonly body: Command; readonly id: CommandId } }
  | { readonly FleetHello: DeviceHello };

const Open = Schema.Union(
  [
    Schema.Struct({ Hello: Hello }),
    Schema.Struct({ Subscribe: Subscribe }),
    Schema.Struct({ Command: Schema.Struct({ body: Command, id: CommandId }) }),
    Schema.Struct({ FleetHello: DeviceHello }),
  ],
  { mode: "oneOf" },
).annotate({ identifier: "Open" });

type Ticket = {
  readonly host: globalThis.ReadonlyArray<number>;
  readonly relay: string;
  readonly token: AuthToken;
};

const Ticket = Schema.Struct({
  host: Schema.Array(
    Schema.Number.annotate({ format: "uint8" })
      .check(Schema.isInt().annotate({ expected: "an integer" }))
      .check(
        Schema.isGreaterThanOrEqualTo(0).annotate({
          expected: "a value greater than or equal to 0",
        }),
      )
      .check(
        Schema.isLessThanOrEqualTo(255).annotate({ expected: "a value less than or equal to 255" }),
      ),
  )
    .check(Schema.isMinLength(32).annotate({ expected: "a value with a length of at least 32" }))
    .check(Schema.isMaxLength(32).annotate({ expected: "a value with a length of at most 32" })),
  relay: Schema.String,
  token: AuthToken,
}).annotate({ identifier: "Ticket" });

export {
  Account,
  Alpn,
  Answer,
  Auth,
  AuthToken,
  Availability,
  Bar,
  Block,
  Body,
  Canvas,
  CanvasEntry,
  CanvasFile,
  CanvasName,
  Catalog,
  Change,
  Change2,
  ChangeGroup,
  ChangeKind,
  Changes,
  ChecklistChange,
  ChecklistEntry,
  ChecklistItem,
  ChecklistStatus,
  Checkout,
  CheckoutLocation,
  Choice,
  ClientName,
  Cloning,
  CloseReason,
  CodeRef,
  Command,
  CommandId,
  CommandResult,
  Completeness,
  Coverage,
  Credential,
  CredentialForm,
  Datum,
  Decision,
  DeviceHello,
  Diagram,
  DiffChange,
  DiffExtent,
  Edge,
  EditId,
  Effort,
  EnrollmentToken,
  Entry,
  Epoch,
  Event,
  EventKind,
  Feed,
  Field,
  FieldKind,
  FieldRole,
  FieldType,
  FileChange,
  FilePatch,
  Finding,
  Finished,
  FleetCommand,
  FleetResult,
  Flow,
  FlowField,
  Group,
  Hello,
  HelloReply,
  Home,
  HomeCommand,
  HomeRefusal,
  HomeResult,
  HomeSync,
  HostEntry,
  HostSeq,
  InstanceCommand,
  InstanceId,
  InstanceRefusal,
  InstanceResult,
  InstanceStatus,
  Instances,
  Invite,
  InviteSecret,
  Issue,
  IssueChange,
  IssueId,
  IssueState,
  Item,
  Kept,
  Kind,
  Literal,
  Locator,
  LogEvent,
  ModelChoice,
  ModelId,
  Models,
  NewIssue,
  Node,
  Note,
  Observation,
  Open,
  Options,
  Origin,
  Output,
  PairRequest,
  Pairing,
  Part,
  Predicate,
  Progress,
  Project,
  ProjectId,
  ProviderInfo,
  PushCommand,
  PushResult,
  PushSubscription,
  Quantifier,
  Question,
  QuestionOption,
  Relation,
  Repository,
  RequestId,
  Resolution,
  Role,
  Rule,
  Section,
  Selection,
  SessionCommand,
  SessionDirectory,
  SessionId,
  SessionRefusal,
  SessionResult,
  Severity,
  Shape,
  SignIn,
  SignInCode,
  SignInCommand,
  SignInRefusal,
  SignInResult,
  SignInStep,
  Signed,
  Snapshot,
  SnapshotDiff,
  SnapshotFile,
  Stage,
  StartAt,
  State,
  Statechart,
  Status,
  Stretch,
  SubagentActivity,
  SubagentEvent,
  SubagentId,
  SubagentOutcome,
  Subscribe,
  Tag,
  Ticket,
  ToolCallId,
  ToolKind,
  ToolOutcome,
  ToolPhase,
  Transition,
  TurnOutcome,
  Unreadable,
  Welcome,
  WorkingCopy,
  Workspace,
  Worktree,
};
