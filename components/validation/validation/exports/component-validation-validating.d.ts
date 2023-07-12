export namespace ExportsComponentValidationValidating {
  export function validate(packageRecords: ProtoEnvelopeBody[]): boolean[];
}
export interface ProtoEnvelopeBody {
  contentBytes: Uint8Array,
  keyId: string,
  signature: string,
}
