#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#import <PDFKit/PDFKit.h>
#import <Vision/Vision.h>

static void recognize_image(NSImage *image, NSInteger page, NSMutableArray *lines) {
  NSRect rect = NSMakeRect(0, 0, image.size.width, image.size.height);
  CGImageRef cg_image = [image CGImageForProposedRect:&rect context:nil hints:nil];
  if (cg_image == nil) return;

  VNRecognizeTextRequest *request = [[VNRecognizeTextRequest alloc]
      initWithCompletionHandler:^(VNRequest *completed, NSError *error) {
        if (error != nil) return;
        for (VNRecognizedTextObservation *observation in completed.results) {
          VNRecognizedText *candidate = [[observation topCandidates:1] firstObject];
          if (candidate == nil || candidate.string.length == 0) continue;
          CGRect box = observation.boundingBox;
          [lines addObject:@{
            @"page": @(page), @"text": candidate.string,
            @"confidence": @(candidate.confidence), @"x": @(box.origin.x),
            @"y": @(box.origin.y), @"width": @(box.size.width), @"height": @(box.size.height)
          }];
        }
      }];
  request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
  request.usesLanguageCorrection = NO;
  NSError *error = nil;
  VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:cg_image options:@{}];
  [handler performRequests:@[ request ] error:&error];
}

int main(int argc, const char *argv[]) {
  @autoreleasepool {
    if (argc != 2) return 64;
    NSURL *url = [NSURL fileURLWithFileSystemRepresentation:argv[1] isDirectory:NO relativeToURL:nil];
    NSMutableArray *lines = [NSMutableArray array];
    if ([[url.pathExtension lowercaseString] isEqualToString:@"pdf"]) {
      PDFDocument *document = [[PDFDocument alloc] initWithURL:url];
      for (NSInteger index = 0; index < document.pageCount; index++) {
        PDFPage *page = [document pageAtIndex:index];
        NSImage *image = [page thumbnailOfSize:NSMakeSize(2200, 3000) forBox:kPDFDisplayBoxMediaBox];
        if (image != nil) recognize_image(image, index + 1, lines);
      }
    } else {
      NSImage *image = [[NSImage alloc] initWithContentsOfURL:url];
      if (image != nil) recognize_image(image, 1, lines);
    }
    NSError *error = nil;
    NSData *json = [NSJSONSerialization dataWithJSONObject:lines options:0 error:&error];
    if (json == nil) return 65;
    fwrite(json.bytes, 1, json.length, stdout);
  }
  return 0;
}
