// eframe updates the Dock icon, but AppKit's About panel uses the named-image
// registry. Register the bundle resource there too, on the application's main
// thread, so both native surfaces resolve the same project artwork.
#import <AppKit/AppKit.h>

void spacemosaic_register_about_icon(void) {
    static NSImage *aboutIcon; // Keep the named image alive after eframe updates the Dock.
    @autoreleasepool {
        NSBundle *bundle = NSBundle.mainBundle;
        NSString *filename = [bundle objectForInfoDictionaryKey:@"CFBundleIconFile"];
        if (filename.length == 0) return; // Running the unbundled Cargo binary.
        NSString *path = [bundle.resourcePath stringByAppendingPathComponent:filename];
        NSImage *icon = [[NSImage alloc] initWithContentsOfFile:path];
        if (icon == nil) return;
        NSImage *previous = [NSImage imageNamed:NSImageNameApplicationIcon];
        [previous setName:nil];
        aboutIcon = icon;
        [aboutIcon setName:NSImageNameApplicationIcon];
        NSApplication.sharedApplication.applicationIconImage = icon;
    }
}
