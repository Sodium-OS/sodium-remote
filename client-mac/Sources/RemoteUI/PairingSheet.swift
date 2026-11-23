// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

/// The pairing sheet: six digits, checked by itself at the sixth;
/// the code never leaves the Mac (SPAKE2 runs with it).
public enum PairingUI {
    /// An `NSHostingController` for an AppKit sheet, 440 pt wide, hugging its content.
    @MainActor public static func makeController(model: PairingModel) -> NSViewController {
        let c = NSHostingController(rootView: PairingSheetView(model: model))
        c.sizingOptions = [.preferredContentSize]
        return c
    }
}

private extension PairingPhase {
    /// A wrong or failed code: the next key clears the boxes.
    var clearsOnNextKey: Bool {
        switch self {
        case .wrong, .failed: return true
        default: return false
        }
    }

    /// Typing does nothing now.
    var blocksInput: Bool {
        switch self {
        case .checking, .tooMany, .expired, .noComputer, .paired: return true
        default: return false
        }
    }

    /// The boxes are dimmed.
    var dimsBoxes: Bool {
        switch self {
        case .checking, .tooMany, .expired, .noComputer: return true
        default: return false
        }
    }

    /// Close and "Try a new code" instead of Cancel.
    var offersNewCode: Bool {
        switch self {
        case .tooMany, .expired, .noComputer: return true
        default: return false
        }
    }

    var isWrong: Bool {
        if case .wrong = self { return true }
        return false
    }
}

@MainActor
private struct PairingSheetView: View {
    @ObservedObject var model: PairingModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Pair a computer")
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(Theme.text)
            if pairedInfo == nil {
                Text("Type the 6-digit code that the computer shows.")
                    .font(.system(size: 13))
                    .foregroundColor(Theme.text2)
                    .padding(.top, 2)
            }
            if let p = pairedInfo {
                PairedContent(model: model, name: p.name, code: p.code)
                    .padding(.top, 14)
            } else {
                CodeEntry(model: model)
                    .padding(.top, 16)
            }
            SheetButtons(model: model, paired: pairedInfo != nil)
                .padding(.top, 18)
        }
        .padding(EdgeInsets(top: 20, leading: 22, bottom: 20, trailing: 22))
        .frame(width: 440, alignment: .leading)
        .background(Theme.dialogBg)
    }

    private var pairedInfo: (name: String, code: String)? {
        if case let .paired(name, code) = model.phase {
            return (name: name, code: code)
        }
        return nil
    }
}

// MARK: - The code

/// The six boxes over a nearly invisible text field that takes the keys, so
/// typing, Backspace and paste behave like any field.
@MainActor
private struct CodeEntry: View {
    @ObservedObject var model: PairingModel
    @State private var entry = ""
    @State private var submitted = false
    @FocusState private var focused: Bool

    var body: some View {
        VStack(spacing: 12) {
            ZStack {
                TextField("", text: $entry)
                    .textFieldStyle(.plain)
                    .font(.system(size: 1))
                    .foregroundColor(Color.clear)
                    .frame(width: 280, height: 44)
                    .opacity(0.011)
                    .focused($focused)
                    .disabled(model.phase.blocksInput)
                    .accessibilityLabel("Pairing code")
                DigitBoxes(digits: model.digits, phase: model.phase, focused: focused)
                    .allowsHitTesting(false)
            }
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
            .onTapGesture {
                focused = true
            }
            CodeMessage(digits: model.digits, phase: model.phase)
                .frame(maxWidth: 370, minHeight: 34, alignment: .top)
                .frame(maxWidth: .infinity)
        }
        .padding(.top, 4)
        .onAppear {
            entry = model.digits
            focused = true
        }
        .onChange(of: entry) { _, new in
            take(new)
        }
        .onChange(of: model.digits) { _, new in
            if entry != new { entry = new }
            if new.count < 6 { submitted = false }
            if new.isEmpty && !model.phase.blocksInput { focused = true }
        }
        .onChange(of: model.phase) { _, new in
            if !new.blocksInput { focused = true }
        }
    }

    /// The field changed: keep digits only, at most six; a wrong code clears
    /// on the next key; the sixth digit submits once.
    private func take(_ raw: String) {
        if model.phase.blocksInput {
            if raw != model.digits { entry = model.digits }
            return
        }
        var typed = raw
        if model.phase.clearsOnNextKey {
            let old = model.digits
            if raw.count > old.count && raw.hasPrefix(old) {
                typed = String(raw.dropFirst(old.count))
            } else if raw.count > old.count {
                typed = raw
            } else {
                typed = ""
            }
            model.phase = .entering
        }
        var digits = String(typed.filter { $0.isASCII && $0.isNumber })
        if digits.count > 6 { digits = String(digits.prefix(6)) }
        if model.digits != digits { model.digits = digits }
        if entry != digits { entry = digits }
        if digits.count == 6 {
            if !submitted {
                submitted = true
                model.actions?.pairingSubmit(code: digits)
            }
        } else {
            submitted = false
        }
    }
}

/// One box per digit, a gap after the third.
@MainActor
private struct DigitBoxes: View {
    let digits: String
    let phase: PairingPhase
    let focused: Bool

    var body: some View {
        let chars: [Character] = Array(digits)
        let entering: Bool = phase == .entering
        return HStack(spacing: 8) {
            ForEach(0..<6, id: \.self) { i in
                DigitBox(text: i < chars.count ? String(chars[i]) : "",
                         current: focused && entering && i == chars.count,
                         error: phase.isWrong,
                         dim: phase.dimsBoxes)
                if i == 2 {
                    Color.clear.frame(width: 12, height: 1)
                }
            }
        }
    }
}

@MainActor
private struct DigitBox: View {
    let text: String
    let current: Bool
    let error: Bool
    let dim: Bool

    var body: some View {
        let ring: Color = error ? Theme.danger : (current ? Theme.accentInk : Color.clear)
        return ZStack {
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Theme.input)
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(ring, lineWidth: 1.5)
            if text.isEmpty {
                if current {
                    Rectangle()
                        .fill(Theme.accentInk)
                        .frame(width: 1.5, height: 20)
                }
            } else {
                Text(text)
                    .font(.system(size: 22, weight: .semibold).monospacedDigit())
                    .foregroundColor(Theme.text)
            }
        }
        .frame(width: 36, height: 44)
        .opacity(dim ? 0.55 : 1)
    }
}

/// The line under the boxes (the board's texts).
@MainActor
private struct CodeMessage: View {
    let digits: String
    let phase: PairingPhase

    var body: some View {
        switch phase {
        case .entering:
            Plain(text: digits.isEmpty ? Text("The code is on the computer, under Settings › Remote › Pair a device.")
                                       : Text("Paste works too. It checks by itself at the sixth digit."))
        case .checking:
            HStack(spacing: 6) {
                Spinner(size: 14)
                Text("Checking with the computer…")
                    .font(.system(size: 12.5))
                    .foregroundColor(Theme.text2)
            }
        case .wrong(let left):
            Marked(symbol: "xmark.octagon", text: CodeMessage.wrongText(left), color: Theme.danger)
        case .tooMany:
            Marked(symbol: "xmark.octagon", text: Text("Too many wrong codes, so the computer withdrew it. Make a new code there."), color: Theme.danger)
        case .expired:
            Marked(symbol: "exclamationmark.triangle", text: Text("This code expired. On the computer, choose New code."), color: Theme.warning)
        case .noComputer:
            Marked(symbol: "exclamationmark.triangle", text: Text("No computer has a pairing window open. On the computer, choose Pair a device."),
                   color: Theme.warning)
        case .failed(let message):
            Marked(symbol: "xmark.octagon", text: Text(verbatim: message), color: Theme.danger)
        case .paired:
            EmptyView()
        }
    }

    static func wrongText(_ left: Int?) -> Text {
        guard let n = left else { return Text("That code didn’t match. Check the computer and try again.") }
        return Text("That code didn’t match. Check the computer and try again. Tries left: \(n).")
    }
}

@MainActor
private struct Plain: View {
    let text: Text

    var body: some View {
        text
            .font(.system(size: 12.5))
            .foregroundColor(Theme.text2)
            .multilineTextAlignment(.center)
            .lineSpacing(2)
            .fixedSize(horizontal: false, vertical: true)
    }
}

@MainActor
private struct Marked: View {
    let symbol: String
    let text: Text
    let color: Color

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: symbol)
                .font(.system(size: 12.5))
            text
                .font(.system(size: 12.5))
                .multilineTextAlignment(.center)
                .lineSpacing(2)
                .fixedSize(horizontal: false, vertical: true)
        }
        .foregroundColor(color)
    }
}

// MARK: - Paired

@MainActor
private struct PairedContent: View {
    @ObservedObject var model: PairingModel
    let name: String
    let code: String

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .center, spacing: 12) {
                CardIcon("checkmark", bg: Theme.successBg, fg: Theme.success)
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(name) is paired")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Theme.text)
                    Text("Its key is saved in this Mac’s Keychain: \(code)")
                        .font(.system(size: 12.5))
                        .foregroundColor(Theme.text2)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            HStack(alignment: .center, spacing: 12) {
                Text("Name here")
                    .font(.system(size: 13))
                    .foregroundColor(Theme.text)
                    .frame(width: 80, alignment: .leading)
                TextField("Name on this Mac", text: $model.nameHere)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 13))
            }
        }
    }
}

// MARK: - Buttons

@MainActor
private struct SheetButtons: View {
    @ObservedObject var model: PairingModel
    let paired: Bool

    var body: some View {
        HStack(spacing: 8) {
            Spacer()
            if paired {
                KitButton("Done", kind: .secondary, height: 30) {
                    model.actions?.pairingFinish(name: model.nameHere, connect: false)
                }
                .keyboardShortcut(.cancelAction)
                KitButton("Connect now", kind: .primary, height: 30) {
                    model.actions?.pairingFinish(name: model.nameHere, connect: true)
                }
                .keyboardShortcut(.defaultAction)
            } else if model.phase.offersNewCode {
                KitButton("Close", kind: .secondary, height: 30) {
                    model.actions?.pairingCancel()
                }
                .keyboardShortcut(.cancelAction)
                KitButton("Try a new code", kind: .primary, height: 30) {
                    model.actions?.pairingTryNew()
                }
                .keyboardShortcut(.defaultAction)
            } else {
                KitButton("Cancel", kind: .secondary, height: 30) {
                    model.actions?.pairingCancel()
                }
                .keyboardShortcut(.cancelAction)
            }
        }
    }
}
