import Foundation
import Network
import CryptoKit
import Darwin

/// TCP listener that accepts the SemOS pairing handshake.
///
/// Uses `Network.framework` so the app can listen on the local network while
/// backgrounded briefly.  The advertised IPv4 is chosen from the active
/// interface (`en0` WiFi preferred, then `bridge100` for USB hotspot).
final class PairingListener: ObservableObject {
    private var listener: NWListener?
    private var connection: NWConnection?
    private var identity: Curve25519.KeyAgreement.PrivateKey?
    private var currentNonceP: Data?
    private var pendingAccept: (() -> Void)?
    private var pendingReject: (() -> Void)?

    /// Start listening. Calls `onReady(ip, port, qrString)` once the socket is
    /// bound and the QR string is known.
    func start(
        identity: Curve25519.KeyAgreement.PrivateKey,
        onReady: @escaping (String, UInt16, String) -> Void,
        onEvent: @escaping (PairingEvent) -> Void
    ) {
        self.identity = identity
        self.currentNonceP = freshNonce()

        do {
            listener = try NWListener(using: .tcp)
        } catch {
            onEvent(.error(.listenerFailed))
            return
        }

        listener?.stateUpdateHandler = { [weak self] state in
            switch state {
            case .ready:
                guard let self = self,
                      let port = self.listener?.port,
                      let ip = self.advertisedIPv4(),
                      let nonceP = self.currentNonceP else {
                    onEvent(.error(.listenerFailed))
                    return
                }
                let qrPayload = QrPayload(
                    phonePublicKey: identity.publicKey,
                    ip: ip,
                    port: UInt16(port.rawValue),
                    nonce: nonceP
                )
                let qrString = hyphenateBase32(crockfordEncode(qrPayload.encode()))
                onReady(self.ipv4String(ip), UInt16(port.rawValue), qrString)
            case .failed(let err):
                onEvent(.error(.handshakeFailed(err.localizedDescription)))
            case .cancelled:
                break
            default:
                break
            }
        }

        listener?.newConnectionHandler = { [weak self] connection in
            self?.handleConnection(connection, onEvent: onEvent)
        }

        listener?.start(queue: .main)
    }

    func stop() {
        pendingAccept = nil
        pendingReject = nil
        listener?.cancel()
        connection?.cancel()
        listener = nil
        connection = nil
        currentNonceP = nil
    }

    /// Call when the human confirms the SAS matches SemOS.
    func confirmSAS() {
        pendingAccept?()
        pendingAccept = nil
        pendingReject = nil
    }

    /// Call when the human rejects the SAS.
    func rejectSAS() {
        pendingReject?()
        pendingAccept = nil
        pendingReject = nil
    }

    // MARK: - Connection handling

    private func handleConnection(_ connection: NWConnection, onEvent: @escaping (PairingEvent) -> Void) {
        self.connection = connection
        connection.start(queue: .main)

        readFrame(connection: connection) { [weak self] result in
            guard let self = self else { return }
            switch result {
            case .failure(let err):
                onEvent(.error(err))
            case .success(let (type, body)):
                guard type == 0x01, body.count == 1 + 32 + 16 else {
                    onEvent(.error(.handshakeFailed("bad HELLO")))
                    return
                }
                let version = body[0]
                guard version == 0x01 else {
                    onEvent(.error(.handshakeFailed("bad version")))
                    return
                }
                let semPub = Data(body[1..<33])
                let nonceS = Data(body[33...])

                guard let identity = self.identity, let nonceP = self.currentNonceP else {
                    onEvent(.error(.handshakeFailed("no identity")))
                    return
                }

                do {
                    let semPublicKey = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: semPub)
                    let sharedSecret = try identity.sharedSecretFromKeyAgreement(with: semPublicKey)

                    let phonePub = identity.publicKey.rawRepresentation
                    let transcript = self.buildTranscript(
                        phonePub: phonePub,
                        nonceP: nonceP,
                        semPub: semPub,
                        nonceS: nonceS
                    )
                    let th = Data(SHA256.hash(data: transcript))

                    let sessionKey = deriveSessionKey(
                        sharedSecret: sharedSecret,
                        nonceP: nonceP,
                        nonceS: nonceS,
                        transcriptHash: th
                    )
                    let sasBytes = deriveSASBytes(
                        sharedSecret: sharedSecret,
                        nonceP: nonceP,
                        nonceS: nonceS,
                        transcriptHash: th
                    )
                    let sas = formatSAS(sasBytes)

                    self.writeFrame(connection: connection, type: 0x02, body: Data([0x01])) { err in
                        if let err = err {
                            onEvent(.error(err))
                            return
                        }

                        self.pendingAccept = { [weak self] in
                            self?.confirmMatch(
                                connection: connection,
                                sessionKey: sessionKey,
                                th: th,
                                identity: identity,
                                semPublicKey: semPublicKey,
                                onEvent: onEvent
                            )
                        }
                        self.pendingReject = {
                            onEvent(.error(.userRejected))
                        }

                        onEvent(.showSAS(sas))
                    }
                } catch {
                    onEvent(.error(.handshakeFailed(error.localizedDescription)))
                }
            }
        }
    }

    private func confirmMatch(
        connection: NWConnection,
        sessionKey: SymmetricKey,
        th: Data,
        identity: Curve25519.KeyAgreement.PrivateKey,
        semPublicKey: Curve25519.KeyAgreement.PublicKey,
        onEvent: @escaping (PairingEvent) -> Void
    ) {
        let mac = hmacSHA256(key: sessionKey, data: Data("confirm-phone".utf8) + th)
        writeFrame(connection: connection, type: 0x03, body: mac) { [weak self] err in
            if let err = err {
                onEvent(.error(err))
                return
            }
            self?.readFrame(connection: connection) { result in
                switch result {
                case .failure(let err):
                    onEvent(.error(err))
                case .success(let (type, body)):
                    guard type == 0x03, body.count == 32 else {
                        onEvent(.error(.handshakeFailed("bad CONFIRM")))
                        return
                    }
                    let expected = hmacSHA256(key: sessionKey, data: Data("confirm-sem".utf8) + th)
                    guard constantTimeEqual(Data(expected), Data(body)) else {
                        onEvent(.error(.authFailed))
                        return
                    }
                    let id = pairingId(
                        phonePublicKey: identity.publicKey.rawRepresentation,
                        semPublicKey: semPublicKey.rawRepresentation
                    )
                    onEvent(.paired(id))
                }
            }
        }
    }

    // MARK: - Helpers

    private func freshNonce() -> Data {
        var bytes = [UInt8](repeating: 0, count: 16)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        return Data(bytes)
    }

    private func advertisedIPv4() -> UInt32? {
        var candidates: [(name: String, addr: UInt32)] = []
        var ifap: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&ifap) == 0, let first = ifap else { return nil }
        defer { freeifaddrs(first) }

        var ptr: UnsafeMutablePointer<ifaddrs>? = first
        while let addr = ptr {
            let name = String(cString: addr.pointee.ifa_name)
            guard let sa = addr.pointee.ifa_addr, sa.pointee.sa_family == AF_INET else {
                ptr = addr.pointee.ifa_next
                continue
            }
            let ip = sa.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { $0.pointee.sin_addr.s_addr }
            if ip.bigEndian == 0x7F000001 { continue }
            candidates.append((name: name, addr: ip))
            ptr = addr.pointee.ifa_next
        }

        let order = ["en0", "bridge100", "en1", "en2"]
        for preferred in order {
            if let match = candidates.first(where: { $0.name == preferred }) {
                return match.addr
            }
        }
        return candidates.first?.addr
    }

    private func ipv4String(_ ip: UInt32) -> String {
        let b = ip.bigEndian
        return "\((b >> 24) & 0xFF).\((b >> 16) & 0xFF).\((b >> 8) & 0xFF).\(b & 0xFF)"
    }

    private func buildTranscript(phonePub: Data, nonceP: Data, semPub: Data, nonceS: Data) -> Data {
        var t = Data("SPv1".utf8)
        t.append(0x01)
        t.append(phonePub)
        t.append(nonceP)
        t.append(semPub)
        t.append(nonceS)
        return t
    }

    // MARK: - Wire framing

    private func readFrame(connection: NWConnection, completion: @escaping (Result<(UInt8, Data), PairingError>) -> Void) {
        readExactly(connection: connection, length: 3) { result in
            switch result {
            case .failure(let err):
                completion(.failure(err))
            case .success(let header):
                let len = UInt16(bigEndian: header.withUnsafeBytes { $0.load(as: UInt16.self) })
                let type = header[2]
                let bodyLen = Int(len) - 1
                guard bodyLen >= 0 && bodyLen <= 125 else {
                    completion(.failure(.handshakeFailed("bad frame length")))
                    return
                }
                self.readExactly(connection: connection, length: bodyLen) { bodyResult in
                    switch bodyResult {
                    case .failure(let err):
                        completion(.failure(err))
                    case .success(let body):
                        completion(.success((type, body)))
                    }
                }
            }
        }
    }

    private func writeFrame(connection: NWConnection, type: UInt8, body: Data, completion: @escaping (PairingError?) -> Void) {
        var frame = Data()
        frame.append(contentsOf: UInt16(1 + body.count).bigEndian.bytes)
        frame.append(type)
        frame.append(body)
        connection.send(content: frame, completion: .contentProcessed { err in
            if err != nil {
                completion(.handshakeFailed("send failed"))
            } else {
                completion(nil)
            }
        })
    }

    private func readExactly(connection: NWConnection, length: Int, completion: @escaping (Result<Data, PairingError>) -> Void) {
        connection.receive(minimumIncompleteLength: length, maximumLength: length) { data, _, isComplete, err in
            if let err = err {
                completion(.failure(.handshakeFailed(err.localizedDescription)))
                return
            }
            guard let data = data, data.count == length else {
                if isComplete {
                    completion(.failure(.handshakeFailed("unexpected EOF")))
                } else {
                    completion(.failure(.handshakeFailed("short read")))
                }
                return
            }
            completion(.success(data))
        }
    }
}

// MARK: - Event type

enum PairingEvent {
    case showSAS(String)
    case paired(String)
    case error(PairingError)
}

// MARK: - Crypto helpers

private func hmacSHA256(key: SymmetricKey, data: Data) -> Data {
    let code = HMAC<SHA256>.authenticationCode(for: data, using: key)
    return Data(code)
}

private func constantTimeEqual(_ a: Data, _ b: Data) -> Bool {
    guard a.count == b.count else { return false }
    var diff: UInt8 = 0
    for i in 0..<a.count {
        diff |= a[i] ^ b[i]
    }
    return diff == 0
}
