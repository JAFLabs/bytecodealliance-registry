use chrono::{DateTime, Utc};
use base64::{Engine as _, engine::{self, general_purpose}, decode};
use std::str::FromStr; // 0.4.15

struct Component;
use bindings::exports::component::validation as validationbindings;
pub use semver::{Version, VersionReq};

use anyhow::Error;
use anyhow::anyhow;

use warg_protocol::{
  package,
  proto_envelope::{ProtoEnvelope, ProtoEnvelopeBody}, 
  SerdeEnvelope,
  registry::{MapCheckpoint, RecordId, LogId, LogLeaf, MapLeaf, PackageId},
};
use warg_crypto::{signing::{self, KeyID, signature}, Decode, hash::{Hash, Sha256, HashAlgorithm, AnyHash}};
use warg_transparency::{log::LogProofBundle, map::MapProofBundle};
use warg_api::v1::proof::ProofError;

#[derive(Debug, Clone)]
// #[serde(rename_all = "camelCase")]
pub struct PackageInfo {
    /// The name of the package.
    pub name: String,
    /// The last known checkpoint of the package.
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    /// The current validation state of the package.
    // #[serde(default)]
    pub state: package::LogState,
}

impl PackageInfo {
  /// Creates a new package info for the given package name and url.
  pub fn new(name: impl Into<String>, 
  ) -> Self {
      Self {
          name: name.into(),
          checkpoint: None,
          state: package::LogState::default(),
      }
  }
}

impl bindings::exports::component::validation::validating::Validating for Component {
    fn validate(
        package_records: Vec<validationbindings::validating::ProtoEnvelopeBody>,
    ) -> Result<Vec<bool>,()> {
      let mut records = Vec::new();
      for record in package_records {
        let decoded = decode(record.content_bytes).unwrap();
        let envelope = ProtoEnvelopeBody {
          content_bytes: decoded,
          key_id: KeyID::from(record.key_id),
          signature: signature::Signature::from_str(&record.signature).unwrap()
        };
        let rec: ProtoEnvelope<package::PackageRecord> = envelope.try_into().unwrap();
        records.push(rec);
      }
      let mut package = PackageInfo::new("foo:bar");
      let mut res = Vec::new();
      for package_record in records {
        let validated = package.state.validate(&package_record);
        match validated {
          Ok(_) => {
            res.push(true);
          }
          _ => {
            res.push(false);
          }
        }
      }
      return Ok(res)
    }
}
bindings::export!(Component);
