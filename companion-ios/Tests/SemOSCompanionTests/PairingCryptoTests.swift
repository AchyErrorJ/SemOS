import XCTest
import CryptoKit
@testable import SemOSCompanion

final class PairingCryptoTests: XCTestCase {
    /// Crockford base32 round-trip using the test-vector payload.
    func testCrockfordRoundTrip() {
        let payload = Data(hex: "5350012fe57da347cd62431528daac5fbb290730fff684afc4cfc2ed90995f58cb3b74c0a8012a1f9050505050505050505050505050505050ba55")!
        let encoded = crockfordEncode(payload)
        let expectedHyphenated = "AD802BZ5-FPHMFKB2-8CAJHPNC-BYXJJ1SG-ZZV89BY4-SZ1EV44S-BXCCPEVM-R2M02AGZ-J1850M2G-A1850M2G-A1850M2G-A18BMN8"
        XCTAssertEqual(hyphenateBase32(encoded), expectedHyphenated)
        let decoded = crockfordDecode(expectedHyphenated)
        XCTAssertEqual(decoded, payload)
    }

    /// CRC-16/CCITT known vector.
    func testCRC16CCITT() {
        let data = Data("123456789".utf8)
        XCTAssertEqual(crc16CCITT(data), 0x29B1)
    }

    /// Reproduce the full handshake vectors from `docs/pairing-v1-test-vectors.md`.
    func testHandshakeVectors() throws {
        let phonePriv = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: Data(hex: "0100000000000000000000000000000000000000000000000000000000000000")!)
        let semPriv = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: Data(hex: "0200000000000000000000000000000000000000000000000000000000000000")!)

        // Public keys.
        XCTAssertEqual(phonePriv.publicKey.rawRepresentation.hex, "2fe57da347cd62431528daac5fbb290730fff684afc4cfc2ed90995f58cb3b74")
        XCTAssertEqual(semPriv.publicKey.rawRepresentation.hex, "2fe57da347cd62431528daac5fbb290730fff684afc4cfc2ed90995f58cb3b74")

        // Shared secret.
        let shared = try phonePriv.sharedSecretFromKeyAgreement(with: semPriv.publicKey)
        XCTAssertEqual(shared.rawRepresentation.hex, "93fea2a7c1aeb62cfd6452ff5badae8bdffcbd7196dc910c89944006d85dbb68")

        // Transcript and hash.
        let nonceP = Data(hex: "50505050505050505050505050505050")!
        let nonceS = Data(hex: "53535353535353535353535353535353")!
        var transcript = Data("SPv1".utf8)
        transcript.append(0x01)
        transcript.append(phonePriv.publicKey.rawRepresentation)
        transcript.append(nonceP)
        transcript.append(semPriv.publicKey.rawRepresentation)
        transcript.append(nonceS)
        XCTAssertEqual(transcript.hex, "53507631012fe57da347cd62431528daac5fbb290730fff684afc4cfc2ed90995f58cb3b74505050505050505050505050505050502fe57da347cd62431528daac5fbb290730fff684afc4cfc2ed90995f58cb3b7453535353535353535353535353535353")
        let th = Data(SHA256.hash(data: transcript))
        XCTAssertEqual(th.hex, "9e1f7c29636ff8c36d18f101435983d6f3d48df84be1f8158d27e4e5b5a7160a")

        // HKDF session key.
        let sessionKey = deriveSessionKey(sharedSecret: shared, nonceP: nonceP, nonceS: nonceS, transcriptHash: th)
        XCTAssertEqual(sessionKey.rawRepresentation.hex, "787bd15f1aeccb296c85854731b8cbab9f6994f2c19e6b4a81597686eeba7fe3")

        // SAS.
        let sasBytes = deriveSASBytes(sharedSecret: shared, nonceP: nonceP, nonceS: nonceS, transcriptHash: th)
        XCTAssertEqual(sasBytes.hex, "96fe2a75")
        XCTAssertEqual(formatSAS(sasBytes), "239413")

        // Pairing id.
        XCTAssertEqual(pairingId(phonePublicKey: phonePriv.publicKey.rawRepresentation,
                                  semPublicKey: semPriv.publicKey.rawRepresentation),
                       "11b7cd9cfd0fdf50")

        // Confirmation MACs.
        let semConfirm = HMAC<SHA256>.authenticationCode(for: Data("confirm-sem".utf8) + th, using: sessionKey)
        XCTAssertEqual(Data(semConfirm).hex, "04d2433da0945fbb7d32d8395d30f29b84659941c4b1d43e5909f1ff35fb2da3")

        let phoneConfirm = HMAC<SHA256>.authenticationCode(for: Data("confirm-phone".utf8) + th, using: sessionKey)
        XCTAssertEqual(Data(phoneConfirm).hex, "dc6f4b95232e6befe12f606a5448eba8df0dbc2163a068e7e2d9e02c5eab984d")
    }
}

extension Data {
    init?(hex: String) {
        var data = Data()
        var idx = hex.startIndex
        while idx < hex.endIndex {
            let next = hex.index(idx, offsetBy: 2, limitedBy: hex.endIndex) ?? hex.endIndex
            guard let byte = UInt8(hex[idx..<next], radix: 16) else { return nil }
            data.append(byte)
            idx = next
        }
        self = data
    }

    var hex: String {
        map { String(format: "%02x", $0) }.joined()
    }
}

extension SymmetricKey {
    var rawRepresentation: Data {
        withUnsafeBytes { Data($0) }
    }
}

extension SharedSecret {
    var rawRepresentation: Data {
        withUnsafeBytes { Data($0) }
    }
}
