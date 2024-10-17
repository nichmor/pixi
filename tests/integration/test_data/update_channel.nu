# Run with e.g. : `nu update_channel.nu dummy_channel_1`
export def main [channel: string] {
    let platforms = ["win-64", "linux-64", "osx-arm64", "osx-64"]
    for platform in $platforms {
        rattler-build build --target-platform $platform --output-dir $channel --recipe $"recipes/($channel).yaml"
    }
    rm -rf $"($channel)/bld"
}
