plugins { id("com.android.application") }

android {
    namespace = "dev.spacemosaic"
    sourceSets.getByName("main").assets.srcDir("../../../assets/i18n")
    compileSdk = 36
    ndkVersion = "29.0.14206865"
    defaultConfig {
        applicationId = "dev.spacemosaic"
        minSdk = 26
        targetSdk = 36
        versionCode = 3
        versionName = "0.1.0-alpha.3"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    buildTypes { release { isMinifyEnabled = false } }
}
