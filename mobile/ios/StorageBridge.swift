import Foundation
import UIKit
import UniformTypeIdentifiers
import Darwin

private enum ProviderFailure: Error { case code(String) }
private func errorCode(_ error: Error) -> String {
    if case ProviderFailure.code(let code) = error { return code }
    let error = error as NSError
    if error.domain == NSCocoaErrorDomain {
        if error.code == NSFileReadNoPermissionError { return "access_revoked" }
        if error.code == NSFileReadNoSuchFileError { return "disappeared" }
        if error.code == NSUserCancelledError { return "cancelled" }
    }
    return "provider_unavailable"
}
private func batch(_ entries: [[String: Any]] = [], done: Bool = true, error: String? = nil) -> [String: Any] {
    ["entries": entries, "done": done, "error": error.map { $0 as Any } ?? NSNull()]
}
private func encoded(_ value: [String: Any]) -> String {
    guard let data = try? JSONSerialization.data(withJSONObject: value),
          data.count <= 256 * 1024, let text = String(data: data, encoding: .utf8) else {
        return "{\"entries\":[],\"done\":true,\"error\":\"message_too_large\"}"
    }
    return text
}
@discardableResult
private func send(_ generation: UInt64, status: String, scope: String? = nil,
                  persisted: Bool = false, error: String? = nil) -> Bool {
    let text = encoded(["generation": generation, "status": status,
        "scope": scope.map { $0 as Any } ?? NSNull(), "entries": [],
        "truncated": false, "persisted": persisted,
        "error": error.map { $0 as Any } ?? NSNull()])
    let data = Data(text.utf8)
    let accepted = data.withUnsafeBytes { spacemosaic_deliver($0.bindMemory(to: UInt8.self).baseAddress, data.count) }
    if accepted {
        DispatchQueue.main.async { if spacemosaic_is_current(generation) { PickerBridge.shared.receive(status) } }
    }
    return accepted
}

private final class MetadataPresenter: NSObject, NSFilePresenter {
    let presentedItemURL: URL?
    let presentedItemOperationQueue = OperationQueue()
    init(_ url: URL) { presentedItemURL = url; super.init(); presentedItemOperationQueue.maxConcurrentOperationCount = 1 }
}
private final class ScopeAccess {
    let url: URL
    private var scoped: Bool
    init(_ url: URL) throws {
        self.url = url; scoped = url.startAccessingSecurityScopedResource()
        let home = URL(fileURLWithPath: NSHomeDirectory()).resolvingSymlinksInPath().path
        let path = url.resolvingSymlinksInPath().path
        guard scoped || path == home || path.hasPrefix(home + "/") else { throw ProviderFailure.code("access_revoked") }
    }
    func close() { if scoped { scoped = false; url.stopAccessingSecurityScopedResource() } }
    deinit { close() }
}
private final class ProviderSession {
    let root: URL
    private let access: ScopeAccess
    let generation: UInt64
    let scan: UInt64
    let presenter: MetadataPresenter
    let coordinator: NSFileCoordinator
    private var enumerator: FileManager.DirectoryEnumerator?
    private var directory: URL?
    private var enumerationError: Error?
    private let keys: Set<URLResourceKey> = [.nameKey, .isDirectoryKey, .isSymbolicLinkKey,
        .fileSizeKey, .fileAllocatedSizeKey, .isUbiquitousItemKey, .ubiquitousItemDownloadingStatusKey, .linkCountKey, .isAliasFileKey]
    init(_ root: URL, generation: UInt64, scan: UInt64) throws {
        access = try ScopeAccess(root)
        self.root = root; self.generation = generation; self.scan = scan
        let metadataPresenter = MetadataPresenter(root)
        presenter = metadataPresenter; coordinator = NSFileCoordinator(filePresenter: metadataPresenter)
        NSFileCoordinator.addFilePresenter(presenter)
    }
    deinit { close(); NSFileCoordinator.removeFilePresenter(presenter); access.close() }
    func close() { enumerator = nil; directory = nil; enumerationError = nil }
    func open(_ url: URL) throws {
        close()
        let rootPath = root.standardizedFileURL.path, path = url.standardizedFileURL.path
        guard url.isFileURL && (path == rootPath || path.hasPrefix(rootPath.hasSuffix("/") ? rootPath : rootPath + "/")) else {
            throw ProviderFailure.code("access_revoked")
        }
        var failure: NSError?
        coordinator.coordinate(readingItemAt: url, options: .immediatelyAvailableMetadataOnly, error: &failure) { coordinated in
            do {
                let values = try coordinated.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
                guard values.isDirectory == true && values.isSymbolicLink != true else { throw ProviderFailure.code("metadata_unavailable") }
                directory = coordinated
                enumerator = FileManager.default.enumerator(at: coordinated, includingPropertiesForKeys: Array(keys),
                    options: [.skipsSubdirectoryDescendants], errorHandler: { [weak self] _, error in
                        self?.enumerationError = error; return false
                    })
                if enumerator == nil { throw ProviderFailure.code("provider_unavailable") }
            } catch { enumerationError = error }
        }
        if let error = (failure as Error?) ?? enumerationError { throw error }
    }
    func next() throws -> [String: Any] {
        guard let directory = directory, let enumerator = enumerator else { throw ProviderFailure.code("provider_unavailable") }
        var entries: [[String: Any]] = [], done = false
        var failure: NSError?
        coordinator.coordinate(readingItemAt: directory, options: .immediatelyAvailableMetadataOnly, error: &failure) { _ in
            while entries.count < 32 {
                guard spacemosaic_is_current(generation) else { enumerationError = ProviderFailure.code("cancelled"); done = true; break }
                guard let child = enumerator.nextObject() as? URL else { done = true; break }
                var entry: [String: Any] = ["name": child.lastPathComponent, "locator": child.absoluteString,
                    "kind": "other", "logical_bytes": NSNull(), "allocated_bytes": NSNull()]
                do {
                    let values = try child.resourceValues(forKeys: keys)
                    guard let directory = values.isDirectory else { throw ProviderFailure.code("metadata_unavailable") }
                    let link = values.isSymbolicLink == true || values.isAliasFile == true
                    let remote = values.isUbiquitousItem == true && values.ubiquitousItemDownloadingStatus != .current
                    let logical = directory || link ? nil : values.fileSize
                    let allocated = directory || link || remote ? nil : values.fileAllocatedSize
                    entry["name"] = values.name ?? child.lastPathComponent
                    entry["kind"] = link ? "link" : (directory ? "directory" : "file")
                    entry["hard_link"] = !directory && !link && (values.linkCount ?? 0) > 1
                    entry["logical_bytes"] = logical.flatMap { $0 >= 0 ? $0 : nil }.map { $0 as Any } ?? NSNull()
                    entry["allocated_bytes"] = allocated.flatMap { $0 >= 0 ? $0 : nil }.map { $0 as Any } ?? NSNull()
                } catch { entry["error"] = errorCode(error) }
                entries.append(entry)
                if (try? JSONSerialization.data(withJSONObject: entries).count) ?? Int.max > 255 * 1024 {
                    entries.removeLast(); enumerationError = ProviderFailure.code("message_too_large"); done = true; break
                }
            }
        }
        let error: Error? = (failure as Error?) ?? enumerationError
        return batch(entries, done: done || error != nil, error: error.map(errorCode))
    }
}
private final class NativeStorage {
    private let lock = NSLock()
    private var selected: (UInt64, URL)?
    private var active: ProviderSession?
    private var cancelledThrough: UInt64 = 0
    func select(_ url: URL, generation: UInt64) {
        lock.lock(); selected = (generation, url); let previous = active; lock.unlock()
        previous?.coordinator.cancel()
    }
    func selectedURL() -> URL? { lock.lock(); defer { lock.unlock() }; return selected?.1 }
    func session(_ generation: UInt64, scan: UInt64) throws -> ProviderSession {
        lock.lock(); let active = self.active; let selected = self.selected; let cancelled = scan <= cancelledThrough; lock.unlock()
        if cancelled { throw ProviderFailure.code("cancelled") }
        if let active = active {
            guard active.generation == generation && active.scan == scan else { throw ProviderFailure.code("provider_busy") }
            return active
        }
        guard let selected = selected, selected.0 == generation, spacemosaic_is_current(generation) else { throw ProviderFailure.code("cancelled") }
        let session = try ProviderSession(selected.1, generation: generation, scan: scan)
        lock.lock()
        if scan <= cancelledThrough { lock.unlock(); throw ProviderFailure.code("cancelled") }
        self.active = session; lock.unlock()
        return session
    }
    func request(_ request: [String: Any]) -> [String: Any] {
        let op = request["op"] as? String ?? "", generation = (request["generation"] as? NSNumber)?.uint64Value ?? 0, scan = (request["scan"] as? NSNumber)?.uint64Value ?? 0
        if op == "cache" {
            let id = (request["id"] as? NSNumber)?.uint64Value ?? 0, json = request["json"] as? Bool ?? true
            return ["path": cacheURL(id, json: json).path]
        }
        if op == "save" {
            let id = (request["id"] as? NSNumber)?.uint64Value ?? 0, json = request["json"] as? Bool ?? true
            let url = URL(fileURLWithPath: request["path"] as? String ?? "")
            guard url.standardizedFileURL == cacheURL(id, json: json).standardizedFileURL else { return [:] }
            DispatchQueue.main.async { PickerBridge.shared.save(id, url: url) }
            return ["accepted": true]
        }
        if op == "copy" || op == "view" {
            let locator = request["locator"] as? String ?? ""
            DispatchQueue.main.async {
                if op == "copy" { UIPasteboard.general.string = locator }
                else { PickerBridge.shared.open(locator) }
            }
            return [:]
        }
        lock.lock(); let active = self.active
        if op == "cancel" { cancelledThrough = max(cancelledThrough,scan) }
        if op == "finish" && active?.generation == generation && active?.scan == scan { self.active = nil }
        lock.unlock()
        if op == "cancel" { if active?.generation == generation && active?.scan == scan { active?.coordinator.cancel() }; return batch() }
        if op == "close" || op == "finish" { if active?.generation == generation && active?.scan == scan { active?.close() }; return batch() }
        do {
            guard spacemosaic_is_current(generation) else { throw ProviderFailure.code("cancelled") }
            let session = try session(generation, scan: scan)
            if op == "open", let locator = request["locator"] as? String, let url = URL(string: locator) {
                try session.open(url); return batch(done: false)
            }
            if op == "next" { return try session.next() }
            return batch(error: "metadata_unavailable")
        } catch { return batch(error: errorCode(error)) }
    }
    private func cacheURL(_ id: UInt64, json: Bool) -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("spacemosaic-export-\(id).\(json ? "json" : "csv")")
    }
}
private let nativeStorage = NativeStorage()
@_cdecl("spacemosaic_provider")
func spacemosaicProvider(_ request: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>? {
    autoreleasepool {
        let data = Data(String(cString: request).utf8)
        guard let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return strdup(encoded(batch(error: "metadata_unavailable"))) }
        return strdup(encoded(nativeStorage.request(value)))
    }
}
@_cdecl("spacemosaic_provider_free")
func spacemosaicProviderFree(_ response: UnsafeMutablePointer<CChar>?) { free(response) }

@MainActor
private final class ExportDelegate: NSObject, UIDocumentPickerDelegate {
    let id: UInt64
    private var completed = false
    init(_ id: UInt64) { self.id = id }
    private func finish(_ outcome: Int32) {
        guard !completed else { return }; completed = true
        spacemosaic_export_done(id, outcome); PickerBridge.shared.exportFinished()
    }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { finish(2) }
    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) { finish(urls.isEmpty ? 0 : 1) }
}
@MainActor
private final class PickerBridge: NSObject, UIDocumentPickerDelegate {
    static let shared = PickerBridge()
    private var browser: AccessibleBrowser?
    private var launcher: UIButton?
    private var picker: UIDocumentPickerViewController?
    private var exportDelegate: ExportDelegate?
    private var generation: UInt64 = 0
    var pickerStatus: String = "waiting"
    static func window() -> UIWindow? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        return scenes.flatMap { $0.windows }.first { $0.isKeyWindow }
            ?? UIApplication.shared.windows.first { $0.isKeyWindow }
    }
    static func presenter() -> UIViewController? {
        guard var root = window()?.rootViewController else { return nil }
        while let presented = root.presentedViewController { root = presented }
        return root
    }
    func configure(_ appearance: Int32, language: String) {
        nativeLanguage = language; nativeAppearance = Int(appearance)
        Self.window()?.overrideUserInterfaceStyle = appearance == 1 ? .light : (appearance == 2 ? .dark : .unspecified)
        launcher?.setTitle(nativeText("accessible_browser"), for: .normal)
        browser?.configure()
    }
    func installLauncher() {
        guard launcher == nil, let root = Self.window()?.rootViewController else { return }
        let button = UIButton(type: .system); button.setTitle(nativeText("accessible_browser"), for: .normal)
        button.titleLabel?.font = .preferredFont(forTextStyle: .body); button.titleLabel?.adjustsFontForContentSizeCategory = true
        button.titleLabel?.numberOfLines = 0; button.backgroundColor = .secondarySystemBackground
        button.addTarget(self, action: #selector(showBrowser), for: .touchUpInside); button.translatesAutoresizingMaskIntoConstraints = false
        root.view.addSubview(button)
        NSLayoutConstraint.activate([button.leadingAnchor.constraint(equalTo: root.view.safeAreaLayoutGuide.leadingAnchor, constant: 12),
            button.trailingAnchor.constraint(equalTo: root.view.safeAreaLayoutGuide.trailingAnchor, constant: -12),
            button.bottomAnchor.constraint(equalTo: root.view.safeAreaLayoutGuide.bottomAnchor, constant: -4),
            button.heightAnchor.constraint(greaterThanOrEqualToConstant: 48)])
        launcher = button
    }
    @objc private func showBrowser() {
        guard let presenter = Self.presenter() else { return }
        let browser = AccessibleBrowser(style: .insetGrouped); self.browser = browser
        presenter.present(UINavigationController(rootViewController: browser), animated: !UIAccessibility.isReduceMotionEnabled)
    }
    func receive(_ status: String) { pickerStatus = status; browser?.reload() }
    func pick(_ generation: UInt64) {
        guard picker == nil && exportDelegate == nil else { send(generation, status: "unavailable", error: "picker_busy"); return }
        guard let presenter = Self.presenter() else { send(generation, status: "unavailable", error: "window_unavailable"); return }
        self.generation = generation; pickerStatus = "waiting"; browser?.reset()
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder], asCopy: false)
        picker.allowsMultipleSelection = false; picker.delegate = self; self.picker = picker
        presenter.present(picker, animated: !UIAccessibility.isReduceMotionEnabled)
    }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { picker = nil; send(generation, status: "cancelled") }
    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        picker = nil
        guard let url = urls.first else { send(generation, status: "cancelled"); return }
        guard spacemosaic_is_current(generation) else { return }
        nativeStorage.select(url, generation: generation)
        send(generation, status: "ready", scope: url.absoluteString)
    }
    func save(_ id: UInt64, url: URL) {
        guard exportDelegate == nil && picker == nil, let presenter = Self.presenter() else { spacemosaic_export_done(id, 0); return }
        let delegate = ExportDelegate(id); exportDelegate = delegate
        let picker = UIDocumentPickerViewController(forExporting: [url], asCopy: true)
        picker.delegate = delegate; presenter.present(picker, animated: !UIAccessibility.isReduceMotionEnabled)
    }
    func exportFinished() { exportDelegate = nil }
    func open(_ locator: String) {
        guard let root = nativeStorage.selectedURL(), let url = URL(string: locator),
            url.isFileURL, let presenter = Self.presenter() else { return }
        let rootPath = root.standardizedFileURL.path, path = url.standardizedFileURL.path
        guard (path == rootPath || path.hasPrefix(rootPath.hasSuffix("/") ? rootPath : rootPath + "/")),
            let access = try? ScopeAccess(root) else {
            let alert = UIAlertController(title: nativeText("open_failed"), message: nativeText("access_revoked"), preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: nativeText("close"), style: .cancel)); presenter.present(alert, animated: false); return
        }
        let sheet = UIActivityViewController(activityItems: [url], applicationActivities: nil)
        sheet.completionWithItemsHandler = { _, _, _, _ in access.close() }
        sheet.popoverPresentationController?.sourceView = presenter.view
        sheet.popoverPresentationController?.sourceRect = CGRect(x: presenter.view.bounds.midX, y: presenter.view.bounds.midY, width: 1, height: 1)
        presenter.present(sheet, animated: !UIAccessibility.isReduceMotionEnabled)
    }
}

private var nativeLanguage: String?
private var nativeAppearance = 0
private func nativeText(_ key: String) -> String {
    let supported = ["en", "ru", "es", "de", "fr", "ja", "pt-BR"]
    let preferred = nativeLanguage ?? Locale.preferredLanguages.first ?? "en"
    let tag = supported.first { preferred.lowercased().hasPrefix($0.lowercased()) } ?? (preferred.hasPrefix("pt") ? "pt-BR" : "en")
    func lookup(_ language: String) -> String? {
        guard let url = Bundle.main.url(forResource: language, withExtension: "json", subdirectory: "i18n"),
            let data = try? Data(contentsOf: url), let catalog = try? JSONSerialization.jsonObject(with: data) as? [String: String] else { return nil }
        return catalog[key]
    }
    return lookup(tag) ?? lookup("en") ?? key
}
private func rustPage(_ request: [String: Any]) -> [String: Any] {
    let request = encoded(request)
    return request.withCString { value in
        guard let response = spacemosaic_result_page(value) else { return [:] }
        defer { spacemosaic_free_result(response) }
        let data = Data(String(cString: response).utf8)
        return (try? JSONSerialization.jsonObject(with: data) as? [String: Any]) ?? [:]
    }
}
@MainActor
private final class AccessibleBrowser: UITableViewController, UISearchResultsUpdating {
    private var page: [String: Any] = [:]
    private var directory = 0, offset = 0, mode = "directory", sort = "size"
    private var offsets: [Int] = [], directories: [Int] = []
    private var allocated = false
    private var loading = false, again = false, sequence = 0
    private let worker = DispatchQueue(label: "dev.spacemosaic.result-reader", qos: .userInitiated)
    private let search = UISearchController(searchResultsController: nil)
    private var entries: [[String: Any]] { page["entries"] as? [[String: Any]] ?? [] }
    override func viewDidLoad() {
        super.viewDidLoad(); tableView.rowHeight = UITableView.automaticDimension; tableView.estimatedRowHeight = 64
        search.obscuresBackgroundDuringPresentation = false; search.searchResultsUpdater = self
        navigationItem.searchController = search; configure(); reload()
    }
    func configure() {
        guard isViewLoaded else { return }
        title = nativeText("accessible_browser"); search.searchBar.placeholder = nativeText("search_hint")
        navigationItem.leftBarButtonItem = UIBarButtonItem(title: nativeText("close"), style: .plain, target: self, action: #selector(close))
        let actionButton = UIBarButtonItem(image: UIImage(systemName: "ellipsis.circle"), style: .plain, target: nil, action: nil)
        actionButton.menu = actions(); actionButton.accessibilityLabel = nativeText("actions")
        navigationItem.rightBarButtonItems = [UIBarButtonItem(title: nativeText("choose"), style: .plain, target: self, action: #selector(choose)), actionButton]
        let modes = UISegmentedControl(items: [nativeText("directory"), nativeText("largest"), nativeText("extensions")])
        modes.selectedSegmentIndex = mode == "directory" ? 0 : (mode == "largest" ? 1 : 2)
        modes.addTarget(self, action: #selector(changeMode(_:)), for: .valueChanged)
        modes.frame = CGRect(x: 16, y: 8, width: max(200, view.bounds.width - 32), height: 40)
        let header = UIView(frame: CGRect(x: 0, y: 0, width: view.bounds.width, height: 56)); header.addSubview(modes)
        modes.autoresizingMask = [.flexibleWidth]; tableView.tableHeaderView = header; reload()
    }
    private func actions() -> UIMenu {
        let commands = ["rescan", "cancel", "export_json", "export_csv", "sort", "allocated", "refresh_result"]
        let appearance = UIMenu(title: nativeText("appearance"), children: ["system","light","dark"].enumerated().map { index,key in
            UIAction(title:nativeText(key),state:nativeAppearance == index ? .on : .off) { _ in _ = rustPage(["command":"appearance","index":index]) }
        })
        let language = UIMenu(title:nativeText("language"),children: ["system","en","ru","es","de","fr","ja","pt-BR"].enumerated().map { index,tag in
            UIAction(title:index == 0 ? nativeText("system") : tag) { _ in _ = rustPage(["command":"language","index":index == 0 ? 7 : index-1]) }
        })
        let items: [UIMenuElement] = commands.map { key in
            UIAction(title: nativeText(key)) { [weak self] _ in
                guard let self = self else { return }
                if key == "allocated" { self.allocated.toggle(); _ = rustPage(["command":"allocated","index":self.allocated ? 1 : 0]); self.reload(); return }
                if key == "sort" { self.sort = self.sort == "size" ? "name" : "size"; self.offset = 0; self.offsets = []; self.reload(); return }
                if key == "refresh_result" { self.reload(); return }
                let command = key == "export_json" ? "json" : (key == "export_csv" ? "csv" : key)
                if key == "rescan" { self.reset() }
                self.sequence += 1; _ = rustPage(["command": command]); DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { self.reload() }
            }
        }
        return UIMenu(children:items + [appearance,language])
    }
    @objc private func close() { dismiss(animated: !UIAccessibility.isReduceMotionEnabled) }
    @objc private func choose() { PickerBridge.shared.pick(spacemosaic_begin()) }
    @objc private func changeMode(_ sender: UISegmentedControl) { mode = ["directory", "largest", "extensions"][sender.selectedSegmentIndex]; offset = 0; offsets = []; reload() }
    func reset() { directory = 0; directories = []; offset = 0; offsets = []; mode = "directory"; search.searchBar.text = ""; sequence += 1; reload() }
    func updateSearchResults(for searchController: UISearchController) { offset = 0; offsets = []; reload() }
    private func navigate(_ index: Int, remember: Bool = true) {
        if remember { directories.append(directory) }
        directory = index; offset = 0; offsets = []; mode = "directory"; search.searchBar.text = ""
        _ = rustPage(["command": "navigate", "index": index, "generation": page["generation"] ?? 0]); reload()
    }
    func reload() {
        guard isViewLoaded, view.window != nil || presentedViewController == nil else { return }
        sequence += 1
        if loading { again = true; return }; loading = true; again = false
        let current = sequence
        let request: [String: Any] = ["directory": directory, "offset": offset, "mode": mode, "sort": sort,
            "query": search.searchBar.text ?? "", "language": nativeLanguage ?? "en", "allocated":allocated]
        worker.async {
            let response = rustPage(request)
            DispatchQueue.main.async {
                self.loading = false
                if current == self.sequence {
                    if let generation = response["generation"] as? NSNumber, !spacemosaic_is_current(generation.uint64Value) { self.reload(); return }
                    self.page = response; self.tableView.reloadData()
                    let status = response["status"] as? String ?? "unavailable"
                    if status == "scanning" || (status == "waiting" && PickerBridge.shared.pickerStatus == "ready") {
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { if self.view.window != nil { self.reload() } }
                    }
                }
                if self.again || current != self.sequence { self.reload() }
            }
        }
    }
    override func numberOfSections(in tableView: UITableView) -> Int { 2 }
    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        if section == 0 { return 2 }
        return entries.count + (offset > 0 ? 1 : 0) + ((page["has_more"] as? Bool == true) ? 1 : 0) + 1
    }
    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = UITableViewCell(style: .subtitle, reuseIdentifier: nil)
        cell.textLabel?.font = .preferredFont(forTextStyle: .body); cell.detailTextLabel?.font = .preferredFont(forTextStyle: .subheadline)
        cell.textLabel?.adjustsFontForContentSizeCategory = true; cell.detailTextLabel?.adjustsFontForContentSizeCategory = true
        cell.textLabel?.numberOfLines = 0; cell.detailTextLabel?.numberOfLines = 0
        if indexPath.section == 0 {
            if indexPath.row == 0 {
                var status = page["status"] as? String ?? "unavailable"
                if status == "waiting" && PickerBridge.shared.pickerStatus != "ready" { status = PickerBridge.shared.pickerStatus }
                cell.textLabel?.text = nativeText(status)
                cell.detailTextLabel?.text = (page["summary"] as? String ?? "") + "\n" + nativeText("mobile_scope") + "\n" + nativeText("size_caveat")
            } else {
                cell.textLabel?.text = page["name"] as? String ?? page["scope"] as? String ?? ""
                if directory != 0 { cell.detailTextLabel?.text = nativeText("back"); cell.accessibilityTraits = .button }
            }
        } else if indexPath.row < entries.count {
            let entry = entries[indexPath.row]
            cell.textLabel?.text = entry["name"] as? String
            cell.detailTextLabel?.text = (entry["size_label"] as? String ?? "") + ((entry["count_label"] as? String).map { " · " + $0 } ?? "")
            if entry["id"] != nil { cell.accessibilityTraits = .button }
            if entry["directory"] as? Bool == true { cell.accessoryType = .disclosureIndicator }
        } else {
            let action = indexPath.row - entries.count
            cell.textLabel?.text = nativeText(offset > 0 && action == 0 ? "previous_page" :
                ((page["has_more"] as? Bool == true && action == (offset > 0 ? 1 : 0)) ? "next_page" : "refresh_result"))
            cell.accessibilityTraits = .button
        }
        return cell
    }
    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: false)
        if indexPath.section == 0 { if indexPath.row == 1, let parent = page["parent"] as? Int { navigate(directories.popLast() ?? parent, remember:false) }; return }
        if indexPath.row >= entries.count {
            let action = indexPath.row - entries.count
            if offset > 0 && action == 0 { offset = offsets.popLast() ?? 0 }
            else if page["has_more"] as? Bool == true && action == (offset > 0 ? 1 : 0) { offsets.append(offset); offset = page["next"] as? Int ?? offset }
            reload(); return
        }
        let entry = entries[indexPath.row]
        guard let id = entry["id"] as? Int else { return }
        if entry["directory"] as? Bool == true { navigate(id); return }
        _ = rustPage(["command": "select", "index": id, "generation": page["generation"] ?? 0])
        let locator = entry["locator"] as? String ?? ""
        var detail = nativeText("logical_bytes") + ": " + (entry["logical_label"] as? String ?? "") + "\n" + nativeText("allocated")
            + ": " + (entry["allocated_label"] as? String ?? "") + "\n" + locator
        if entry["hard_link"] as? Bool == true { detail += "\n" + nativeText("hard_link_note") }
        if let error = entry["error"] as? String { detail += "\n" + nativeText(error) }
        let alert = UIAlertController(title: entry["name"] as? String, message: detail, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: nativeText("copy_path"), style: .default) { _ in UIPasteboard.general.string = locator })
        alert.addAction(UIAlertAction(title: nativeText("reveal"), style: .default) { _ in PickerBridge.shared.open(locator) })
        alert.addAction(UIAlertAction(title: nativeText("close"), style: .cancel))
        present(alert, animated: !UIAccessibility.isReduceMotionEnabled)
    }
}
@_cdecl("spacemosaic_pick_folder")
func spacemosaicPickFolder(_ generation: UInt64) { DispatchQueue.main.async { PickerBridge.shared.pick(generation) } }
@_cdecl("spacemosaic_install_accessibility")
func spacemosaicInstallAccessibility() { DispatchQueue.main.async { PickerBridge.shared.installLauncher() } }
@_cdecl("spacemosaic_configure")
func spacemosaicConfigure(_ appearance: Int32, _ language: UnsafePointer<CChar>) {
    let tag = String(cString: language); DispatchQueue.main.async { PickerBridge.shared.configure(appearance, language: tag) }
}
