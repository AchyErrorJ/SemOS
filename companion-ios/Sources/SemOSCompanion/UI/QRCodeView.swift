import SwiftUI
import CoreImage.CIFilterBuiltins

#if canImport(UIKit)
import UIKit
#endif

#if canImport(AppKit)
import AppKit
#endif

/// Render a QR code from arbitrary string data.
struct QRCodeView: View {
    let data: String
    let side: CGFloat

    var body: some View {
        #if canImport(UIKit)
        Image(uiImage: qrImage(from: data, side: side) ?? UIImage())
            .resizable()
            .interpolation(.none)
            .frame(width: side, height: side)
        #else
        Image(nsImage: qrImage(from: data, side: side) ?? NSImage())
            .resizable()
            .interpolation(.none)
            .frame(width: side, height: side)
        #endif
    }

    #if canImport(UIKit)
    private func qrImage(from string: String, side: CGFloat) -> UIImage? {
        guard let cg = cgImage(from: string, side: side) else { return nil }
        return UIImage(cgImage: cg)
    }
    #else
    private func qrImage(from string: String, side: CGFloat) -> NSImage? {
        guard let cg = cgImage(from: string, side: side) else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: side, height: side))
    }
    #endif

    private func cgImage(from string: String, side: CGFloat) -> CGImage? {
        let context = CIContext()
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(string.utf8)
        filter.correctionLevel = "M"
        guard let outputImage = filter.outputImage else { return nil }
        let scaleX = side / outputImage.extent.size.width
        let scaleY = side / outputImage.extent.size.height
        let transformed = outputImage.transformed(by: CGAffineTransform(scaleX: scaleX, y: scaleY))
        return context.createCGImage(transformed, from: transformed.extent)
    }
}

#if DEBUG
struct QRCodeView_Previews: PreviewProvider {
    static var previews: some View {
        QRCodeView(data: "AD802BZ5-FPHMFKB2", side: 200)
    }
}
#endif
