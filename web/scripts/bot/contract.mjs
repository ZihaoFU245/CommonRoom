import { readFileSync } from "node:fs";
import Ajv from "ajv";

export const schema = JSON.parse(
  readFileSync(
    new URL("../../../docs/bot-v1.schema.json", import.meta.url),
    "utf8",
  ),
);
const ajv = new Ajv({ allErrors: true, schemaId: "auto" });
ajv.addSchema(schema);
const validators = new Map();
export function validate(name, value) {
  let validator = validators.get(name);
  if (!validator) {
    validator = ajv.compile({ $ref: `${schema.$id}#/definitions/${name}` });
    validators.set(name, validator);
  }
  if (!validator(value))
    throw new Error(`Invalid ${name}: ${ajv.errorsText(validator.errors)}`);
  return value;
}
