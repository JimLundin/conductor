import {
  Array as Arr,
  Codegen,
  Effect,
  FileSystem,
  JsonSchema,
  Path,
  Schema,
  SchemaRepresentation,
  Struct,
} from "effect";
import { NodeRuntime, NodeServices } from "@effect/platform-node";

const schemaModule = (): Codegen.Module => {
  const module = new Codegen.Module();
  module.provideImport({
    attributes: "",
    imported: "*",
    kind: "Namespace",
    local: "Schema",
    module: "effect/Schema",
    typeOnly: false,
  });
  return module;
};

const printModule = (module: Readonly<Codegen.Module>): string => {
  const generated = module.finalize();
  const declarations = generated.declarations.flatMap(({ fragments }) =>
    fragments.map((fragment) => fragment.replace(/^export /u, "")),
  );
  return `${[
    'import { Schema } from "effect";',
    ...generated.imports,
    ...declarations,
    ...generated.body,
  ].join("\n\n")}\n`;
};

const print = (source: Readonly<JsonSchema.JsonSchema>): string => {
  const { definitions } = JsonSchema.fromSchemaDraft2020_12(source);
  const names = Object.keys(definitions);
  if (!Arr.isArrayNonEmpty(names)) {
    return "";
  }
  const schemas = SchemaRepresentation.fromJsonSchemaMultiDocument(
    {
      definitions,
      dialect: "draft-2020-12",
      schemas: Arr.map(names, (name) => ({ $ref: `#/$defs/${name}` })),
    },
    { closeObjects: true },
  );
  const document = SchemaRepresentation.toRepresentations(Arr.map(schemas, Struct.get("ast")));
  const module = schemaModule();
  const bindings = module.addSchemas(Object.fromEntries(Arr.zip(names, document.representations)), {
    references: document.references,
  });
  module.add(Codegen.code`export { ${Codegen.join([...bindings.values()], ", ")} }`);
  return printModule(module);
};

const generate = Effect.gen(function* generateSchemas() {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  const published = yield* path.fromFileUrl(
    new URL("../../crates/link/link.schema.json", import.meta.url),
  );
  const source = yield* fs.readFileString(published);
  const document = yield* Schema.decodeUnknownEffect(Schema.fromJsonString(Schema.JsonObject))(
    source,
  );
  const generated = yield* Effect.try(() => print(document));
  const target = yield* path.fromFileUrl(new URL("../core/src/schemas/index.ts", import.meta.url));
  yield* fs.makeDirectory(path.dirname(target), { recursive: true });
  yield* fs.writeFileString(target, generated);
});

NodeRuntime.runMain(Effect.provide(generate, NodeServices.layer));
