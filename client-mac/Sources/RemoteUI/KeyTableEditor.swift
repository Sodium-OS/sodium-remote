// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

/// The "Mac keys" table in Settings › Input:
/// the table, editable. A click on "Sent as" records the next key
/// combination (Esc cancels); a changed row says so; "Reset to the table"
/// brings back; "Add a key…" records both sides. With PC keys the
/// table is dimmed (it isn't sent).
@MainActor
struct KeyTableEditor: View {
    @ObservedObject var model: AppModel
    @StateObject private var rec = KeyTableRecording()

    init(model: AppModel) {
        self.model = model
    }

    /// The Mac side recorded so far by "Add a key…", while its Sent-as side is recorded.
    private var addingFrom: KeyChordModel? {
        if case .addTo(let from) = rec.target { return from }
        return nil
    }

    private var adding: Bool {
        rec.target == .addFrom || addingFrom != nil
    }

    var body: some View {
        let isPC: Bool = model.settings.keyboard == .pc
        let table: KeyTable = model.keyTable
        let changed: Set<String> = table.changedIDs(against: model.defaultKeyTable)
        let firstID: String? = table.rules.first?.id
        return VStack(alignment: .leading, spacing: 0) {
            SectionHeader("Mac keys")
                .padding(.horizontal, 4)
                .padding(.bottom, 8)
            VStack(spacing: 0) {
                KeyTableHead()
                Rectangle().fill(Theme.hair2).frame(height: 1).padding(.horizontal, 16)
                ForEach(table.rules) { rule in
                    VStack(spacing: 0) {
                        if rule.id != firstID {
                            Rectangle().fill(Theme.hair2).frame(height: 1).padding(.horizontal, 16)
                        }
                        KeyRuleRow(rule: rule,
                                   changed: changed.contains(rule.id),
                                   recording: rec.target == .sentAs(rule.id),
                                   onEdit: { rec.start(.sentAs(rule.id), model: model) },
                                   onCancel: { rec.stop() })
                    }
                }
                if adding {
                    Rectangle().fill(Theme.hair2).frame(height: 1).padding(.horizontal, 16)
                    AddKeyRow(from: addingFrom, onCancel: { rec.stop() })
                }
            }
            .padding(.bottom, 4)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.raised2))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.hair2, lineWidth: 1))
            .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            HStack {
                SettingsLink("Add a key…", symbol: "plus") {
                    rec.start(.addFrom, model: model)
                }
                Spacer(minLength: 8)
                SettingsLink("Reset to the table") {
                    rec.stop()
                    model.settings.keyTable = nil
                }
            }
            .padding(.top, 6)
            .padding(.horizontal, 4)
            Text("⌃ + letter and ⌥ pass through as Ctrl and Alt.")
                .font(.system(size: 12))
                .foregroundColor(Theme.text2)
                .padding(.top, 4)
                .padding(.horizontal, 4)
        }
        .opacity(isPC ? 0.45 : 1)
        .disabled(isPC)
        .onChange(of: isPC) { _, pc in
            if pc { rec.stop() }
        }
        .onDisappear {
            rec.stop()
        }
    }
}

@MainActor
private struct KeyTableHead: View {
    var body: some View {
        HStack(spacing: 12) {
            Text("On this Mac").frame(width: KeyTableMetrics.fromWidth, alignment: .leading)
            Text("Sent as")
            Spacer(minLength: 0)
        }
        .font(.system(size: 11.5, weight: .semibold))
        .foregroundColor(Theme.text3)
        .padding(.horizontal, 16)
        .frame(height: 28)
    }
}

enum KeyTableMetrics {
    /// "On this Mac" takes about 46% of the panel.
    static let fromWidth: CGFloat = 210
}

@MainActor
private struct KeyRuleRow: View {
    let rule: KeyRuleModel
    let changed: Bool
    let recording: Bool
    let onEdit: () -> Void
    let onCancel: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Text(KeyNames.from(rule))
                .font(.system(size: 12.5, weight: .medium))
                .foregroundColor(Theme.text)
                .lineLimit(1)
                .frame(width: KeyTableMetrics.fromWidth, alignment: .leading)
            if recording {
                KeyRecordingField(prompt: "Press the keys to send…")
                KitButton("Cancel", kind: .secondary, height: 28, action: onCancel)
            } else {
                SwiftUI.Button(action: onEdit) {
                    HStack(spacing: 0) {
                        Text(KeyNames.to(rule))
                            .font(.system(size: 12.5))
                            .foregroundColor(Theme.text)
                            .lineLimit(1)
                        if changed {
                            Text(" · changed")
                                .font(.system(size: 11.5))
                                .foregroundColor(Theme.warning)
                        }
                        Spacer(minLength: 0)
                    }
                    .frame(minHeight: 30)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Click, then press the keys to send")
            }
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 30)
        .background(recording ? Theme.hover : Color.clear)
    }
}

/// The row "Add a key…" records into: the Mac side first, then Sent as.
@MainActor
private struct AddKeyRow: View {
    /// The recorded Mac side (nil: still recording it).
    let from: KeyChordModel?
    let onCancel: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Group {
                if let f = from {
                    Text(KeyNames.mac(f))
                        .font(.system(size: 12.5, weight: .medium))
                        .foregroundColor(Theme.text)
                        .lineLimit(1)
                } else {
                    KeyRecordingField(prompt: "Press the keys on this Mac…")
                }
            }
            .frame(width: KeyTableMetrics.fromWidth, alignment: .leading)
            if from != nil {
                KeyRecordingField(prompt: "Press the keys to send…")
            } else {
                Text("Then the keys to send")
                    .font(.system(size: 12.5))
                    .foregroundColor(Theme.text3)
                Spacer(minLength: 0)
            }
            KitButton("Cancel", kind: .secondary, height: 28, action: onCancel)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 4)
        .frame(minHeight: 30)
        .background(Theme.hover)
    }
}

/// A field that looks focused while keys are recorded into it.
@MainActor
private struct KeyRecordingField: View {
    let prompt: String

    var body: some View {
        HStack(spacing: 6) {
            Rectangle().fill(Theme.accentInk).frame(width: 1.5, height: 13)
            Text(prompt)
                .font(.system(size: 12.5))
                .foregroundColor(Theme.text3)
                .lineLimit(1)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .frame(height: 28)
        .frame(maxWidth: .infinity)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.input))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Theme.accentInk, lineWidth: 1.5))
    }
}

/// Recording keys for the table: a local event monitor that swallows key
/// events while a cell records (so they reach neither the sheet nor the
/// stream). A modifier pressed and released alone records the bare modifier.
@MainActor
final class KeyTableRecording: ObservableObject {
    enum Target: Equatable {
        case idle
        /// Recording a new Sent-as for the rule with this id.
        case sentAs(String)
        /// "Add a key…": the Mac side.
        case addFrom
        /// "Add a key…": the Sent-as side, for this Mac side.
        case addTo(KeyChordModel)
    }

    @Published var target: Target = .idle
    private var monitor: Any?
    private var pendingModifier: KeyChordModel?
    private weak var model: AppModel?

    /// The modifier bits a chord can carry (NSEvent raw values).
    private static let chordFlags: UInt = KeyMods.flagCommand | KeyMods.flagControl | KeyMods.flagOption | KeyMods.flagShift
    private static let escape: UInt16 = 0x35
    private static let capsLock: UInt16 = 0x39

    init() {}

    func start(_ t: Target, model m: AppModel) {
        model = m
        target = t
        pendingModifier = nil
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .flagsChanged]) { [weak self] event in
            let isKeyDown: Bool = event.type == .keyDown
            let code: UInt16 = event.keyCode
            let flags: UInt = event.modifierFlags.rawValue
            let swallow: Bool = MainActor.assumeIsolated {
                self?.handle(keyDown: isKeyDown, keyCode: code, flags: flags) ?? false
            }
            return swallow ? nil : event
        }
    }

    func stop() {
        if let m = monitor {
            NSEvent.removeMonitor(m)
        }
        monitor = nil
        pendingModifier = nil
        if target != .idle { target = .idle }
    }

    /// Returns true to swallow the event.
    private func handle(keyDown: Bool, keyCode: UInt16, flags: UInt) -> Bool {
        if target == .idle { return false }
        if keyDown {
            pendingModifier = nil
            if keyCode == KeyTableRecording.escape && flags & KeyTableRecording.chordFlags == 0 {
                stop()
                return true
            }
            if let chord = KeyRecorder.chord(keyCode: keyCode, flags: flags) {
                recorded(chord)
            }
            return true
        }
        // flagsChanged: a modifier went down or up.
        if keyCode == KeyTableRecording.capsLock { return true }
        let held: UInt = flags & (KeyTableRecording.chordFlags | KeyMods.flagFunction)
        if held != 0 {
            if pendingModifier == nil, let c = KeyRecorder.chord(keyCode: keyCode, flags: flags), c.isBareModifier {
                pendingModifier = c
            }
        } else if let c = pendingModifier {
            pendingModifier = nil
            recorded(c)
        }
        return true
    }

    private func recorded(_ chord: KeyChordModel) {
        guard let m = model else {
            stop()
            return
        }
        switch target {
        case .idle:
            return
        case .sentAs(let id):
            var table = m.keyTable
            if let i = table.rules.firstIndex(where: { $0.id == id }) {
                var rule = table.rules[i]
                rule.to = [chord]
                table.upsert(rule)
                m.settings.keyTable = table
            }
            stop()
        case .addFrom:
            target = .addTo(chord)
        case .addTo(let from):
            var table = m.keyTable
            table.upsert(KeyRuleModel(when: .any, from: from, to: [chord]))
            m.settings.keyTable = table
            stop()
        }
    }
}
