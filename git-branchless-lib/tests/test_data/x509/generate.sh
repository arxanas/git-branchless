#!/bin/bash

# Adapted from:
# https://gitlab.com/gitlab-org/gitlab-development-kit/-/blob/d2087a9b973b1b8c61a4787aafd4f2ae3b968d54/doc/howto/x509_self_signed_commits.md
#
# Requires openssl 1.1, 3.X does not work

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

openssl genrsa -out ca.key 4096

openssl req \
  -new \
  -x509 \
  -subj "/CN=example.com" \
  -days 36500 \
  -key ca.key \
  -out ca.crt

openssl genrsa -out git.key 4096

openssl req \
  -new \
  -subj "/CN=example.com" \
  -key git.key  \
  -out git.csr

openssl x509 -req -days 36500 -in git.csr -CA ca.crt -CAkey ca.key -extfile <(
    echo "subjectAltName = email:test@example.com,email:test2@example.com"
    echo "keyUsage = critical,digitalSignature"
    echo "subjectKeyIdentifier = hash"
    echo "authorityKeyIdentifier = keyid"
    echo "crlDistributionPoints=DNS:example.com,URI:http://example.com/crl.pem"
) -set_serial 1 -out git.crt

openssl pkcs12 -export -inkey git.key -in git.crt -name test -out git.p12

openssl pkcs12 -export -inkey ca.key -in ca.crt -name test2 -out ca.p12

HOME=$SCRIPT_DIR
GNUPGHOME=$SCRIPT_DIR

gpgsm --import ca.p12
gpgsm --import git.p12

echo disable-crl-checks:0:1 | gpgconf --change-options gpgsm
gpgsm --list-keys | grep 'sha1 fpr' | awk -F 'sha1 fpr: ' '{ print $2 " S relax" }' >> ~/trustlist.txt

