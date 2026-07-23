import Foundation
import CryptoKit

/// Errors that can occur during the pairing flow.
enum PairingError: Error, Equatable {
    case identityLoadFailed
    case qrEncodeFailed
    case listenerFailed
    case addressResolutionFailed
    case handshakeFailed(String)
    case authFailed
    case userRejected
}

/// The current phase of the pairing UI.
enum PairingPhase: Equatable {
    case idle
    case listening(ip: String, port: UInt16, qrString: String)
    case awaitingConfirmation(sas: String)
    case paired(pairingId: String)
    case failed(PairingError)
}

/// Fixed layout of the binary QR payload, phone → SemOS.
struct QrPayload {
    var version: UInt8
    var phonePublicKey: Data
    var ip: UInt32
    var port: UInt16
    var nonce: Data

    static let length = 59

    init(phonePublicKey: Curve25519.KeyAgreement.PublicKey, ip: UInt32, port: UInt16, nonce: Data) {
        self.version = 0x01
        self.phonePublicKey = phonePublicKey.rawRepresentation
        self.ip = ip
        self.port = port
        self.nonce = nonce
    }

    /// Encode to the 59-byte binary payload described in `docs/pairing-v1.md` §3.
    func encode() -> Data {
        var out = Data()
        out.append(contentsOf: "SP".utf8)
        out.append(version)
        out.append(phonePublicKey)
        out.append(contentsOf: ip.bigEndian.bytes)
        out.append(contentsOf: port.bigEndian.bytes)
        out.append(nonce)
        let crc = UInt16(crc16CCITT(out)).bigEndian
        out.append(contentsOf: crc.bytes)
        assert(out.count == QrPayload.length)
        return out
    }
}

extension FixedWidthInteger {
    var bytes: [UInt8] {
        withUnsafeBytes(of: self) { Array($0) }
    }
}
