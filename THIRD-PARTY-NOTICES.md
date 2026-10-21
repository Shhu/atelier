# Third-party notices

Atelier's binary embeds the following works, under their own licenses.

## JPEG encoding (Independent JPEG Group)

This software is based in part on the work of the Independent JPEG Group.

CMYK JPEGs are written by the `jpeg-encoder` crate (MIT or Apache-2.0),
whose forward DCT is ported from mozjpeg, itself derived from the IJG's
libjpeg.

## Little CMS

Color management by Little CMS (https://www.littlecms.com), compiled into
the binary through the `lcms2` crate.

```
MIT License

Copyright (c) 2023 Marti Maria Saguer

Permission is hereby granted, free of charge, to any person obtaining
a copy of this software and associated documentation files (the 
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to 
permit persons to whom the Software is furnished to do so, subject
to the following conditions:

The above copyright notice and this permission notice shall be 
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF 
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE 
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

## FOGRA39L Coated color profile

`assets/icc/FOGRA39L_coated.icc`, the default working CMYK, embedded in
the binary. Generated from the OpenICC data by colord's
cd-create-profile; from the CTAN "colorprofiles" package. The profile
states: "This profile is free of known copyright restrictions".

```
FOGRA39L_coated
Copyright:  Kai-Uwe Behrmann <www.behrmann.name>
            Marti Maria <www.littlecms.com>
            Photogamut <www.photogamut.org>
            Graeme Gill <www.argyllcms.com>
            ColorSolutions <www.basICColor.com>



The zlib/libpng License

This software is provided 'as-is', without any express or implied
warranty. In no event will the authors be held liable for any damages
arising from the use of this software.

Permission is granted to anyone to use this software for any purpose,
including commercial applications, and to alter it and redistribute it
freely, subject to the following restrictions:

  1. The origin of this software must not be misrepresented; you must not
  claim that you wrote the original software. If you use this software
  in a product, an acknowledgment in the product documentation would be
  appreciated but is not required.

  2. Altered source versions must be plainly marked as such, and must not be
  misrepresented as being the original software.

  3. This notice may not be removed or altered from any source
  distribution.

```

## Geist fonts

The interface fonts (assets/fonts) are Geist and Geist Mono by Vercel, under
the SIL Open Font License 1.1: see assets/fonts/OFL.txt.

## ONNX Runtime and DirectML (Windows build)

The Windows executable embeds ONNX Runtime (MIT license, Microsoft) with
DirectML (Microsoft's redistributable license) and the Microsoft Visual C++
runtime libraries it needs. `packaging/windows/fetch-runtime.sh` downloads
them with their license files, which go with the build.

## Rust crates

Atelier is built with many Rust crates, each under its own license (MIT,
Apache-2.0, BSD, zlib, MPL-2.0, Unicode-3.0 and similar permissive licenses, choosing the permissive option where a crate offers several); see
Cargo.lock for the list.
