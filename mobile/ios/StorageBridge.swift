import Foundation
import UIKit
import UniformTypeIdentifiers

// The Rust/winit entry point owns UIApplication and its delegate. Do not create a second one.
@_cdecl("spacemosaic_pick_folder")
func spacemosaicPickFolder(_ generation: UInt64) {
    DispatchQueue.main.async { PickerBridge.shared.pick(generation) }
}

private func send(_ generation: UInt64, status: String, scope: String? = nil,
                  entries: [[String: Any]] = [], truncated: Bool = false,
                  persisted: Bool = false, error: String? = nil) {
    let message: [String: Any] = [
        "generation": generation, "status": status,
        "scope": scope.map { $0 as Any } ?? NSNull(), "entries": entries,
        "truncated": truncated, "persisted": persisted,
        "error": error.map { $0 as Any } ?? NSNull()
    ]
    guard let data = try? JSONSerialization.data(withJSONObject: message) else { return }
    if data.count > 256 * 1024 {
        send(generation, status: "unavailable", error: "message_too_large")
        return
    }
    let accepted = data.withUnsafeBytes { bytes in
        spacemosaic_deliver(bytes.bindMemory(to: UInt8.self).baseAddress, data.count)
    }
    if accepted {
        DispatchQueue.main.async {
            if spacemosaic_is_current(generation) { PickerBridge.shared.receive(message) }
        }
    }
}

private final class MetadataPresenter: NSObject, NSFilePresenter {
    let presentedItemURL: URL?
    let presentedItemOperationQueue = OperationQueue()
    init(_ url: URL) {
        presentedItemURL = url
        super.init()
        presentedItemOperationQueue.maxConcurrentOperationCount = 1
    }
}

@MainActor
private final class PickerBridge: NSObject, UIDocumentPickerDelegate {
    static let shared = PickerBridge()
    private var browser: AccessibleBrowser?
    private var lastMessage: [String: Any]?
    private var launcher: UIButton?
    func configure(_ appearance: Int32, language: String) {
        nativeLanguage = language
        Self.window()?.overrideUserInterfaceStyle = appearance == 1 ? .light : (appearance == 2 ? .dark : .unspecified)
        launcher?.setTitle(nativeText("accessible_browser"), for: .normal)
        browser?.title = nativeText("accessible_browser")
        browser?.tableView.reloadData()
    }

    static func window() -> UIWindow? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        // winit 0.30 uses legacy UIWindow without a scene manifest.
        return scenes.flatMap { $0.windows }.first { $0.isKeyWindow }
            ?? UIApplication.shared.windows.first { $0.isKeyWindow }
    }

    func installLauncher() {
        guard launcher == nil, let root = Self.window()?.rootViewController else { return }
        let button = UIButton(type: .system)
        button.setTitle(nativeText("accessible_browser"), for: .normal)
        button.titleLabel?.font = .preferredFont(forTextStyle: .body)
        button.titleLabel?.adjustsFontForContentSizeCategory = true
        button.backgroundColor = .secondarySystemBackground
        button.addTarget(self, action: #selector(showBrowser), for: .touchUpInside)
        button.translatesAutoresizingMaskIntoConstraints = false
        root.view.addSubview(button)
        NSLayoutConstraint.activate([
            button.leadingAnchor.constraint(equalTo: root.view.safeAreaLayoutGuide.leadingAnchor, constant: 12),
            button.trailingAnchor.constraint(equalTo: root.view.safeAreaLayoutGuide.trailingAnchor, constant: -12),
            button.bottomAnchor.constraint(equalTo: root.view.safeAreaLayoutGuide.bottomAnchor, constant: -4),
            button.heightAnchor.constraint(greaterThanOrEqualToConstant: 48)
        ])
        launcher = button
    }

    @objc private func showBrowser() {
        guard let root = Self.window()?.rootViewController else { return }
        var presenter = root
        while let next = presenter.presentedViewController { presenter = next }
        let browser = AccessibleBrowser(style: .insetGrouped)
        browser.message = lastMessage
        browser.onChoose = { [weak self] in self?.pick(spacemosaic_begin()) }
        self.browser = browser
        presenter.present(UINavigationController(rootViewController: browser), animated: !UIAccessibility.isReduceMotionEnabled)
    }

    func receive(_ message: [String: Any]) {
        lastMessage = message
        browser?.message = message
        browser?.tableView.reloadData()
    }
    private var generation: UInt64 = 0
    private var picker: UIDocumentPickerViewController?
    private let worker = DispatchQueue(label: "dev.spacemosaic.metadata", qos: .userInitiated)
    // This flag lives on the main queue. Do not enqueue unlimited blocked-provider work.
    private var workerBusy = false

    func pick(_ generation: UInt64) {
        guard picker == nil && !workerBusy else {
            send(generation, status: "unavailable", error: "picker_busy")
            return
        }
        guard let window = Self.window(),
              var presenter = window.rootViewController else {
            send(generation, status: "unavailable", error: "window_unavailable")
            return
        }
        while let next = presenter.presentedViewController { presenter = next }
        lastMessage = nil
        browser?.message = nil
        browser?.tableView.reloadData()
        self.generation = generation
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder], asCopy: false)
        picker.allowsMultipleSelection = false
        picker.delegate = self
        self.picker = picker
        presenter.present(picker, animated: !UIAccessibility.isReduceMotionEnabled)
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        picker = nil
        send(generation, status: "cancelled")
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        picker = nil
        let session = generation
        guard let url = urls.first else { send(session, status: "cancelled"); return }
        workerBusy = true
        worker.async {
            Self.enumerate(url, generation: session)
            DispatchQueue.main.async { self.workerBusy = false }
        }
    }

    nonisolated private static func enumerate(_ url: URL, generation: UInt64) {
        guard url.startAccessingSecurityScopedResource() else {
            send(generation, status: "denied", error: "access_revoked")
            return
        }
        defer { url.stopAccessingSecurityScopedResource() }
        let presenter = MetadataPresenter(url)
        NSFileCoordinator.addFilePresenter(presenter)
        defer { NSFileCoordinator.removeFilePresenter(presenter) }
        var coordinationError: NSError?
        var entries: [[String: Any]] = []
        var truncated = false
        var providerError: Error?
        // Immediate metadata only prevents coordination from materializing cloud files.
        NSFileCoordinator(filePresenter: presenter).coordinate(readingItemAt: url,
            options: .immediatelyAvailableMetadataOnly, error: &coordinationError) { coordinated in
            let keys: Set<URLResourceKey> = [.nameKey, .isDirectoryKey, .isSymbolicLinkKey,
                .fileSizeKey, .fileAllocatedSizeKey, .isUbiquitousItemKey,
                .ubiquitousItemDownloadingStatusKey]
            guard let enumerator = FileManager.default.enumerator(at: coordinated,
                includingPropertiesForKeys: Array(keys), options: [.skipsSubdirectoryDescendants],
                errorHandler: { _, error in providerError = error; return false }) else {
                providerError = CocoaError(.fileReadUnknown)
                return
            }
            for case let child as URL in enumerator {
                if entries.count == 32 { truncated = true; break }
                do {
                    let values = try child.resourceValues(forKeys: keys)
                    let directory = values.isDirectory == true
                    let link = values.isSymbolicLink == true
                    let remote = values.isUbiquitousItem == true
                        && values.ubiquitousItemDownloadingStatus != .current
                    // Avoid claiming local allocated bytes for unmaterialized cloud items or links.
                    let logical = directory || link ? nil : values.fileSize
                    let allocated = directory || link || remote ? nil : values.fileAllocatedSize
                    entries.append([
                        "name": values.name ?? child.lastPathComponent,
                        "locator": child.absoluteString,
                        "directory": directory,
                        "logical_bytes": logical.flatMap { $0 >= 0 ? $0 : nil }.map { $0 as Any } ?? NSNull(),
                        "allocated_bytes": allocated.flatMap { $0 >= 0 ? $0 : nil }.map { $0 as Any } ?? NSNull()
                    ])
                } catch { providerError = error }
            }
        }
        let error = coordinationError ?? providerError as NSError?
        let status = error == nil ? "ready" :
            (error?.domain == NSCocoaErrorDomain && error?.code == NSFileReadNoPermissionError
                ? "denied" : "unavailable")
        // Persistence is not claimed: the metadata browser retains no URL/bookmark after scope ends.
        send(generation, status: status, scope: url.absoluteString, entries: entries,
             truncated: truncated, error: error == nil ? nil : "provider_unavailable")
    }
}


private var nativeLanguage: String?

private func nativeText(_ key: String) -> String {
    let supported = ["en", "ru", "es", "de", "fr", "ja", "pt-BR"]
    let preferred = nativeLanguage ?? Locale.preferredLanguages.first ?? "en"
    let tag = supported.first { preferred.lowercased().hasPrefix($0.lowercased()) }
        ?? (preferred.hasPrefix("pt") ? "pt-BR" : "en")
    func lookup(_ language: String) -> String? {
        guard let url = Bundle.main.url(forResource: language, withExtension: "json", subdirectory: "i18n"),
              let data = try? Data(contentsOf: url),
              let catalog = try? JSONSerialization.jsonObject(with: data) as? [String: String] else { return nil }
        return catalog[key]
    }
    return lookup(tag) ?? lookup("en") ?? key
}

private final class AccessibleBrowser: UITableViewController {
    var message: [String: Any]?
    var onChoose: (() -> Void)?
    override func viewDidLoad() {
        super.viewDidLoad()
        title = nativeText("accessible_browser")
        navigationItem.leftBarButtonItem = UIBarButtonItem(title: nativeText("close"), style: .plain,
            target: self, action: #selector(close))
        tableView.rowHeight = UITableView.automaticDimension
        tableView.estimatedRowHeight = 60
    }
    @objc private func close() { dismiss(animated: !UIAccessibility.isReduceMotionEnabled) }
    override func numberOfSections(in tableView: UITableView) -> Int { 2 }
    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        if section == 0 { return 2 }
        return (message?["entries"] as? [[String: Any]])?.count ?? 0
    }
    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = UITableViewCell(style: .subtitle, reuseIdentifier: nil)
        cell.textLabel?.font = .preferredFont(forTextStyle: .body)
        cell.detailTextLabel?.font = .preferredFont(forTextStyle: .subheadline)
        cell.textLabel?.adjustsFontForContentSizeCategory = true
        cell.detailTextLabel?.adjustsFontForContentSizeCategory = true
        cell.textLabel?.numberOfLines = 0
        cell.detailTextLabel?.numberOfLines = 0
        if indexPath.section == 0 {
            cell.textLabel?.text = nativeText(indexPath.row == 0 ? "choose" : "mobile_scope")
            if indexPath.row == 0 { cell.accessibilityTraits = .button }
            else if let status = message?["status"] as? String {
                cell.detailTextLabel?.text = nativeText(status == "ready" ? "ready" : status)
            }
        } else if let entry = (message?["entries"] as? [[String: Any]])?[indexPath.row] {
            cell.textLabel?.text = entry["name"] as? String
            let size = (entry["logical_bytes"] as? NSNumber).map { NumberFormatter.localizedString(from: $0, number: .decimal) }
                ?? nativeText("unknown")
            cell.detailTextLabel?.text = "\(nativeText("logical_bytes")): \(size)\n\(entry["locator"] as? String ?? "")"
        }
        return cell
    }
    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: false)
        if indexPath.section == 0 && indexPath.row == 0 { onChoose?() }
    }
}

@_cdecl("spacemosaic_install_accessibility")
func spacemosaicInstallAccessibility() {
    DispatchQueue.main.async { PickerBridge.shared.installLauncher() }
}

@_cdecl("spacemosaic_configure")
func spacemosaicConfigure(_ appearance: Int32, _ language: UnsafePointer<CChar>) {
    let tag = String(cString: language)
    DispatchQueue.main.async { PickerBridge.shared.configure(appearance, language: tag) }
}
