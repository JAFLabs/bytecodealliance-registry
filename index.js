// const fetch = require("fetch")
import { hashing } from "./components/hash-checkpoint/hash-checkpoint/hash_checkpoint.js"
import { validating } from "./components/validation/validation/validation.js"

let enc = new TextEncoder()
const doStuff = async () => {
  let res = await fetch("http://127.0.0.1:8090/v1/fetch/checkpoint")
  let body = await res.json()
  const { contents, keyId, signature } = body
  let root = hashing.hashCheckpoint(contents, keyId, signature)
  let logs_resp = await fetch("http://127.0.0.1:8090/v1/fetch/logs", {
    method: "POST",
    headers: {
      "Content-Type": "application/json"
    },
    body: JSON.stringify({
      "root": root,
      "packages": {"sha256:0221a0a7acb17c1cde7aa86b8abc8b3ec67f2eab1caae459d261b723b5dacbcd": null}
    })
  });
  let logs = await logs_resp.json()
  let log_records = Object.values(logs.packages)[0]
  let validated = validating.validate(log_records.map(l => ({
    contentBytes: enc.encode(l.contentBytes),
    keyId: l.keyId,
    signature: l.signature
  })))
  return validated
}

doStuff().then(res => console.log({res}))