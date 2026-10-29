// UIKit host for the iOS build, using the UIScene life cycle. Rust owns the process
// `main` and calls `ferrsphen_ios_main`; GPUI renders into a view controller that the
// scene's window hosts, and a CADisplayLink delivers frames on demand.

import UIKit

@_cdecl("ferrsphen_ios_main")
public func ferrsphenIOSMain() -> Int32 {
    UIApplicationMain(CommandLine.argc, CommandLine.unsafeArgv, nil, NSStringFromClass(AppDelegate.self))
}

@objc(FerrsphenAppDelegate)
final class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        configurationForConnecting session: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        let configuration = UISceneConfiguration(name: nil, sessionRole: session.role)
        configuration.delegateClass = SceneDelegate.self
        return configuration
    }

    func applicationWillTerminate(_ application: UIApplication) {
        gpui_ios_will_terminate(nil)
    }
}

@objc(FerrsphenSceneDelegate)
final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    private var gpuiWindow: UnsafeMutableRawPointer?
    private var displayLink: CADisplayLink?

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let scene = scene as? UIWindowScene else { return }
        // GPUI creates only the view controller; this scene owns the UIWindow.
        gpui_ios_set_embedded()
        ferrsphen_ios_start()
        guard let gpuiWindow = gpui_ios_get_window(),
              let controller = gpui_ios_view_controller(gpuiWindow)
        else {
            fatalError("GPUI did not create a window")
        }
        self.gpuiWindow = gpuiWindow
        let window = UIWindow(windowScene: scene)
        window.rootViewController = Unmanaged<UIViewController>.fromOpaque(controller).takeUnretainedValue()
        window.makeKeyAndVisible()
        self.window = window
    }

    func sceneWillEnterForeground(_ scene: UIScene) {
        gpui_ios_will_enter_foreground(nil)
        startDisplayLink()
    }

    func sceneDidBecomeActive(_ scene: UIScene) {
        gpui_ios_did_become_active(nil)
    }

    func sceneWillResignActive(_ scene: UIScene) {
        gpui_ios_will_resign_active(nil)
    }

    func sceneDidEnterBackground(_ scene: UIScene) {
        gpui_ios_did_enter_background(nil)
        stopDisplayLink()
    }

    func sceneDidDisconnect(_ scene: UIScene) {
        stopDisplayLink()
    }

    private func startDisplayLink() {
        guard displayLink == nil, let gpuiWindow else { return }
        let link = CADisplayLink(target: self, selector: #selector(renderFrame))
        link.add(to: .main, forMode: .common)
        displayLink = link
        gpui_ios_set_frame_waker(gpuiWindow, { context in
            guard let context else { return }
            Unmanaged<SceneDelegate>.fromOpaque(context).takeUnretainedValue().displayLink?.isPaused = false
        }, Unmanaged.passUnretained(self).toOpaque())
    }

    private func stopDisplayLink() {
        if let gpuiWindow {
            gpui_ios_set_frame_waker(gpuiWindow, nil, nil)
        }
        displayLink?.invalidate()
        displayLink = nil
    }

    @objc private func renderFrame() {
        if !gpui_ios_request_frame(gpuiWindow) {
            displayLink?.isPaused = true
        }
    }
}
