import Foundation
import CryptoKit

/// Crockford base32 alphabet.
private let crockfordAlphabet = Array("0123456789ABCDEFGHJKMNPQRSTVWXYZ")

/// Map ASCII characters to their Crockford 5-bit values.
private func crockfordValue(_ c: Character) -> UInt8? {
    switch c {
    case "0", "O", "o": return 0
    case "1", "I", "i", "L", "l": return 1
    case "2": return 2
    case "3": return 3
    case "4": return 4
    case "5": return 5
    case "6": return 6
    case "7": return 7
    case "8": return 8
    case "9": return 9
    case "A", "a": return 10
    case "B", "b": return 11
    case "C", "c": return 12
    case "D", "d": return 13
    case "E", "e": return 14
    case "F", "f": return 15
    case "G", "g": return 16
    case "H", "h": return 17
    case "J", "j": return 18
    case "K", "k": return 19
    case "M", "m": return 20
    case "N", "n": return 21
    case "P", "p": return 22
    case "Q", "q": return 23
    case "R", "r": return 24
    case "S", "s": return 25
    case "T", "t": return 26
    case "V", "v": return 27
    case "W", "w": return 28
    case "X", "x": return 29
    case "Y", "y": return 30
    case "Z", "z": return 31
    default: return nil
    }
}

/// Encode bytes to Crockford base32 (uppercase, no padding).
func crockfordEncode(_ data: Data) -> String {
    var bits = 0
    var bitsLeft = 0
    var out = ""
    for byte in data {
        bits = (bits << 8) | Int(byte)
        bitsLeft += 8
        while bitsLeft >= 5 {
            bitsLeft -= 5
            let index = (bits >> bitsLeft) & 0x1F
            out.append(crockfordAlphabet[index])
        }
    }
    if bitsLeft > 0 {
        let index = (bits << (5 - bitsLeft)) & 0x1F
        out.append(crockfordAlphabet[index])
    }
    return out
}

/// Decode Crockford base32. Ignores hyphens, accepts aliases.
func crockfordDecode(_ s: String) -> Data? {
    var bits = 0
    var bitsLeft = 0
    var out = Data()
    for c in s {
        if c == "-" { continue }
        guard let val = crockfordValue(c) else { return nil }
        bits = (bits << 5) | Int(val)
        bitsLeft += 5
        if bitsLeft >= 8 {
            bitsLeft -= 8
            out.append(UInt8((bits >> bitsLeft) & 0xFF))
        }
    }
    if bitsLeft > 0 && (bits & ((1 << bitsLeft) - 1)) != 0 {
        return nil
    }
    return out
}

/// CRC-16/CCITT-FALSE over `data`.
func crc16CCITT(_ data: Data) -> UInt16 {
    var crc: UInt16 = 0xFFFF
    for byte in data {
        crc ^= UInt16(byte) << 8
        for _ in 0..<8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021
            } else {
                crc <<= 1
            }
        }
    }
    return crc
}

/// Format a base32 string with a hyphen every 8 characters.
func hyphenateBase32(_ s: String) -> String {
    var out = ""
    for (i, c) in s.enumerated() {
        if i > 0 && i % 8 == 0 {
            out.append("-")
        }
        out.append(c)
    }
    return out
}

/// Derive the pairing session key from the X25519 shared secret.
func deriveSessionKey(sharedSecret: SharedSecret, nonceP: Data, nonceS: Data, transcriptHash: Data) -> SymmetricKey {
    var salt = Data()
    salt.append(nonceP)
    salt.append(nonceS)
    var info = Data("semos-pair-v1 session".utf8)
    info.append(transcriptHash)
    return sharedSecret.x963DerivedKey(
        inputKeyingMaterial: sharedSecret,
        salt: salt,
        sharedInfo: info,
        outputByteCount: 32
    )
}

/// Derive the 4-byte SAS value.
func deriveSASBytes(sharedSecret: SharedSecret, nonceP: Data, nonceS: Data, transcriptHash: Data) -> Data {
    var salt = Data()
    salt.append(nonceP)
    salt.append(nonceS)
    var info = Data("semos-pair-v1 sas".utf8)
    info.append(transcriptHash)
    let key = sharedSecret.x963DerivedKey(
        inputKeyingMaterial: sharedSecret,
        salt: salt,
        sharedInfo: info,
        outputByteCount: 4
    )
    return key.withUnsafeBytes { Data($0) }
}

/// Convert 4 big-endian SAS bytes to a 6-digit zero-padded decimal string.
func formatSAS(_ sasBytes: Data) -> String {
    precondition(sasBytes.count == 4)
    let value = UInt32(bigEndian: sasBytes.withUnsafeBytes { $0.load(as: UInt32.self) })
    return String(format: "%06d", value % 1_000_000)
}

/// Compute the pairing id: first 8 bytes of SHA256("semos-pair-id" || phone_pub || sem_pub), hex.
func pairingId(phonePublicKey: Data, semPublicKey: Data) -> String {
    var input = Data("semos-pair-id".utf8)
    input.append(phonePublicKey)
    input.append(semPublicKey)
    let hash = SHA256.hash(data: input)
    return hash.prefix(8).map { String(format: "%02x", $0) }.joined()
}

// MARK: - SharedSecret helper

extension SharedSecret {
    /// HKDF-SHA256 expand helper matching CryptoKit's `x963DerivedKey` semantics.
    fileprivate func x963DerivedKey(
        inputKeyingMaterial: SharedSecret,
        salt: Data,
        sharedInfo: Data,
        outputByteCount: Int
    ) -> SymmetricKey {
        let ikm = self.withUnsafeBytes { Data($0) }
        return HKDF<SHA256>.deriveKey(
            inputKeyMaterial: .init(data: ikm),
            salt: salt,
            info: sharedInfo,
            outputByteCount: outputByteCount
        )
    }
}

// MARK: - Data helpers

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
}
