target/bundle-common/Info.plist: res/Info.plist Cargo.toml Cargo.lock
	mkdir -p target/bundle-common
	sed "s/####VERSION####/`cargo read-manifest | jq -r '.version'`/" res/Info.plist > $@

target/gyroflow-ofx-linux.zip: target/release/libgyroflow_ofx.so LICENSE README.md target/bundle-common/Info.plist Makefile
	rm -Rf target/gyroflow-ofx-linux
	rm -f target/gyroflow-ofx-linux.zip
	mkdir -p target/gyroflow-ofx-linux/Gyroflow.ofx.bundle/Contents/Linux-x86-64
	cp target/release/libgyroflow_ofx.so target/gyroflow-ofx-linux/Gyroflow.ofx.bundle/Contents/Linux-x86-64/Gyroflow.ofx
	cp target/bundle-common/Info.plist LICENSE README.md target/gyroflow-ofx-linux/Gyroflow.ofx.bundle/Contents/
	cd target/gyroflow-ofx-linux && zip -r ../gyroflow-ofx-linux.zip .

target/gyroflow-ofx-macosx.dmg: target/x86_64-apple-darwin/release/libgyroflow_ofx.dylib target/aarch64-apple-darwin/release/libgyroflow_ofx.dylib LICENSE README.md target/bundle-common/Info.plist Makefile
	rm -Rf target/gyroflow-ofx-macosx
	rm -f target/gyroflow-ofx-macosx.dmg
	mkdir -p target/gyroflow-ofx-macosx/Gyroflow.ofx.bundle/Contents/MacOS

	lipo target/{x86_64,aarch64}-apple-darwin/release/libgyroflow_ofx.dylib -create -output target/gyroflow-ofx-macosx/Gyroflow.ofx.bundle/Contents/MacOS/Gyroflow.dylib
	cp target/bundle-common/Info.plist LICENSE README.md target/gyroflow-ofx-macosx/Gyroflow.ofx.bundle/Contents/

	codesign -vvvv --strict --options=runtime --timestamp --force -s ${SIGNING_FINGERPRINT} target/gyroflow-ofx-macosx/Gyroflow.ofx.bundle/Contents/MacOS/Gyroflow.dylib
	mv target/gyroflow-ofx-macosx/Gyroflow.ofx.bundle/Contents/MacOS/Gyroflow.dylib target/gyroflow-ofx-macosx/Gyroflow.ofx.bundle/Contents/MacOS/Gyroflow.ofx

	codesign -vvvv --deep --strict --options=runtime --timestamp --force -s ${SIGNING_FINGERPRINT} target/gyroflow-ofx-macosx/Gyroflow.ofx.bundle
	codesign -vvvv --deep --verify target/gyroflow-ofx-macosx/Gyroflow.ofx.bundle

	ln -sf /Library/OFX/Plugins "target/gyroflow-ofx-macosx/"
	hdiutil create "target/gyroflow-ofx-macosx.dmg" -volname "Gyroflow-ofx" -fs HFS+ -srcfolder "target/gyroflow-ofx-macosx/" -ov -format UDZO -imagekey zlib-level=9

	codesign -vvvv --strict --options=runtime --timestamp --force -s ${SIGNING_FINGERPRINT} target/gyroflow-ofx-macosx.dmg
	codesign -vvvv --deep --verify target/gyroflow-ofx-macosx.dmg

target/gyroflow-ofx-windows.zip: target/release/gyroflow_ofx.dll LICENSE README.md target/bundle-common/Info.plist Makefile
	rm -Rf target/gyroflow-ofx-windows
	rm -f target/gyroflow-ofx-windows.zip
	mkdir -p target/gyroflow-ofx-windows/Gyroflow.ofx.bundle/Contents/Win64
	cp target/release/gyroflow_ofx.dll target/gyroflow-ofx-windows/Gyroflow.ofx.bundle/Contents/Win64/Gyroflow.ofx
	cp target/bundle-common/Info.plist LICENSE README.md target/gyroflow-ofx-windows/Gyroflow.ofx.bundle/Contents/
	cd target/gyroflow-ofx-windows && zip -r ../gyroflow-ofx-windows.zip .
