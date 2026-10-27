plugins { id("com.android.application") }

// Public version comes from Cargo; versionCode is a separately increasing Android counter.
val appVersion = Regex("(?m)^version = \"([^\"]+)\"$")
    .find(rootProject.file("../../Cargo.toml").readText())!!.groupValues[1]
val androidVersionCode = providers.gradleProperty("spacemosaicVersionCode").get().toInt()
require(androidVersionCode in 1..2100000000) { "Invalid spacemosaicVersionCode" }
val signingNames = listOf("KEYSTORE", "STORE_PASSWORD", "KEY_ALIAS", "KEY_PASSWORD")
val signingValues = signingNames.associateWith {
    providers.environmentVariable("SPACEMOSAIC_ANDROID_$it").orNull
}

android {
    namespace = "dev.spacemosaic"
    sourceSets.getByName("main").assets.srcDir("../../../assets/i18n")
    compileSdk = 36
    ndkVersion = "29.0.14206865"
    defaultConfig {
        applicationId = "dev.spacemosaic"
        minSdk = 26
        targetSdk = 36
        versionCode = androidVersionCode
        versionName = appVersion
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    signingConfigs {
        create("distribution") {
            if (signingValues.values.all { !it.isNullOrBlank() }) {
                storeFile = file(signingValues.getValue("KEYSTORE")!!)
                storePassword = signingValues.getValue("STORE_PASSWORD")
                keyAlias = signingValues.getValue("KEY_ALIAS")
                keyPassword = signingValues.getValue("KEY_PASSWORD")
            }
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("distribution")
        }
    }
}

// Even direct Gradle release builds must never fall back to a debug/unsigned APK.
val validateDistributionSigning = tasks.register("validateDistributionSigning") {
    doLast {
        val missing = signingNames.filter { signingValues[it].isNullOrBlank() }
        check(missing.isEmpty()) {
            "Release signing requires " + missing.joinToString { "SPACEMOSAIC_ANDROID_$it" } +
                ". See docs/android-distribution.md. Debug builds need no credentials."
        }
        check(file(signingValues.getValue("KEYSTORE")!!).isFile) {
            "Release keystore is not a readable file. See docs/android-distribution.md."
        }
    }
}
tasks.configureEach {
    if (name == "preReleaseBuild") dependsOn(validateDistributionSigning)
}
