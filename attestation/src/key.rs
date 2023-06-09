// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! This module implements ECDSA (NIST P-256 curve) keys related functions. You
//! can export private key to a DER format or create a certificate with
//! extension for TLS-based remote attestation.

use anyhow::Result;
use sgx_crypto::ecc::{EcKeyPair, EcPublicKey};

/// Validation days of cert for TLS connection.
const CERT_VALID_DAYS: i64 = 90i64;

/// NistP256KeyPair stores a pair of ECDSA (private, public) key based on the
/// NIST P-256 curve (a.k.a secp256r1).
pub struct NistP256KeyPair {
    inner: EcKeyPair,
}

impl NistP256KeyPair {
    /// Generate a ECDSA key pair.
    pub fn new() -> Result<Self> {
        let inner = EcKeyPair::create()?;
        Ok(Self { inner })
    }

    pub fn pub_k(&self) -> EcPublicKey {
        self.inner.public_key()
    }

    pub(crate) fn private_key_into_der(&self) -> Vec<u8> {
        use bit_vec::BitVec;
        use yasna::construct_der;
        use yasna::models::ObjectIdentifier;
        use yasna::Tag;

        // Construct useful OIDs.
        let ec_public_key_oid = ObjectIdentifier::from_slice(&[1, 2, 840, 10045, 2, 1]);
        let prime256v1_oid = ObjectIdentifier::from_slice(&[1, 2, 840, 10045, 3, 1, 7]);

        let pub_key_bytes = self.public_key_into_bytes();
        let prv_key_bytes = self.private_key_into_bytes();

        // Construct private key in DER.
        construct_der(|writer| {
            writer.write_sequence(|writer| {
                writer.next().write_u8(0);
                writer.next().write_sequence(|writer| {
                    writer.next().write_oid(&ec_public_key_oid);
                    writer.next().write_oid(&prime256v1_oid);
                });
                let inner_key_der = construct_der(|writer| {
                    writer.write_sequence(|writer| {
                        writer.next().write_u8(1);
                        writer.next().write_bytes(&prv_key_bytes);
                        writer.next().write_tagged(Tag::context(1), |writer| {
                            writer.write_bitvec(&BitVec::from_bytes(&pub_key_bytes));
                        });
                    });
                });
                writer.next().write_bytes(&inner_key_der);
            });
        })
    }

    /// create_cert_with_extension makes a self-signed x509-v3 cert with SGX
    /// attestation report as extensions.
    /// @reference [Internet X.509 Public Key Infrastructure Certificate and
    /// Certificate Revocation List (CRL) Profile][1]
    ///
    /// [1]: https://tools.ietf.org/pdf/rfc5280.pdf
    pub(crate) fn create_cert_with_extension(
        &self,
        issuer: &str,
        subject: &str,
        payload: &[u8],
    ) -> Vec<u8> {
        use crate::cert::*;
        use bit_vec::BitVec;
        use chrono::TimeZone;
        use num_bigint::BigUint;
        use std::time::SystemTime;
        use std::time::UNIX_EPOCH;
        #[allow(unused_imports)]
        use std::untrusted::time::SystemTimeEx;
        use yasna::construct_der;
        use yasna::models::{ObjectIdentifier, UTCTime};

        // Construct useful OIDs.
        let ecdsa_with_sha256_oid = ObjectIdentifier::from_slice(&[1, 2, 840, 10045, 4, 3, 2]);
        let common_name_oid = ObjectIdentifier::from_slice(&[2, 5, 4, 3]);
        let ec_public_key_oid = ObjectIdentifier::from_slice(&[1, 2, 840, 10045, 2, 1]);
        let prime256v1_oid = ObjectIdentifier::from_slice(&[1, 2, 840, 10045, 3, 1, 7]);
        let comment_oid = ObjectIdentifier::from_slice(&[2, 16, 840, 1, 113_730, 1, 13]);

        let pub_key_bytes = self.public_key_into_bytes();

        // UNIX_EPOCH is the earliest time stamp. This unwrap should constantly succeed.
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let issue_ts = chrono::Utc.timestamp_opt(now.as_secs() as i64, 0).unwrap();

        // This is guaranteed to be a valid duration.
        let expire = now + chrono::Duration::days(CERT_VALID_DAYS).to_std().unwrap();
        let expire_ts = chrono::Utc
            .timestamp_opt(expire.as_secs() as i64, 0)
            .unwrap();

        // Construct certificate with payload in extension in DER.
        let tbs_cert_der = construct_der(|writer| {
            let version = 2i8;
            let serial = 1u8;
            let cert_sign_algo = asn1_seq!(ecdsa_with_sha256_oid.clone());
            let issuer = asn1_seq!(asn1_seq!(asn1_seq!(
                common_name_oid.clone(),
                issuer.to_owned()
            )));
            let valid_range = asn1_seq!(
                UTCTime::from_datetime(&issue_ts),
                UTCTime::from_datetime(&expire_ts),
            );
            let subject = asn1_seq!(asn1_seq!(asn1_seq!(
                common_name_oid.clone(),
                subject.to_string(),
            )));
            let pub_key = asn1_seq!(
                asn1_seq!(ec_public_key_oid, prime256v1_oid,),
                BitVec::from_bytes(&pub_key_bytes),
            );
            let sgx_ra_cert_ext = asn1_seq!(asn1_seq!(comment_oid, payload.to_owned()));
            let tbs_cert = asn1_seq!(
                version,
                serial,
                cert_sign_algo,
                issuer,
                valid_range,
                subject,
                pub_key,
                sgx_ra_cert_ext,
            );
            TbsCert::dump(writer, tbs_cert);
        });

        // There will be serious problems if this call fails. We might as well
        // panic in this case, thus unwrap()
        let sig = self
            .inner
            .private_key()
            .sign(tbs_cert_der.as_slice())
            .unwrap();

        let sig_der = yasna::construct_der(|writer| {
            writer.write_sequence(|writer| {
                let sig = sig.signature();
                let mut sig_x = sig.x;
                sig_x.reverse();
                let mut sig_y = sig.y;
                sig_y.reverse();
                writer.next().write_biguint(&BigUint::from_slice(&sig_x));
                writer.next().write_biguint(&BigUint::from_slice(&sig_y));
            });
        });

        yasna::construct_der(|writer| {
            writer.write_sequence(|writer| {
                writer.next().write_der(tbs_cert_der.as_slice());
                CertSignAlgo::dump(writer.next(), asn1_seq!(ecdsa_with_sha256_oid.clone()));
                writer
                    .next()
                    .write_bitvec(&BitVec::from_bytes(sig_der.as_slice()));
            });
        })
    }

    fn public_key_into_bytes(&self) -> Vec<u8> {
        // The first byte must be 4, which indicates the uncompressed encoding.
        let mut pub_key_bytes: Vec<u8> = vec![4];
        let public_key = self.pub_k().public_key();
        pub_key_bytes.extend(public_key.gx.iter().rev());
        pub_key_bytes.extend(public_key.gy.iter().rev());
        pub_key_bytes
    }

    fn private_key_into_bytes(&self) -> Vec<u8> {
        let mut prv_key_bytes: Vec<u8> = vec![];
        let private_key = self.inner.private_key().private_key();
        prv_key_bytes.extend(private_key.r.iter().rev());
        prv_key_bytes
    }
}
