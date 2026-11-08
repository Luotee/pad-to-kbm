// Controller art (spec 4.4, "B 貼紙" sticker style with contour shoulders). v3.7.0: DualSense, Switch Pro and
// Switch 2 Pro are traced from product photos (bodies, two-tone panels, seams, keys at the photo positions).

function circ(cx, cy, r) { return 'M' + (cx - r) + ',' + cy + ' a' + r + ',' + r + ' 0 1,0 ' + 2 * r + ',0 a' + r + ',' + r + ' 0 1,0 ' + -2 * r + ',0 Z'; }
function rr(x, y, w, h, r) { return 'M' + (x + r) + ',' + y + ' h' + (w - 2 * r) + ' a' + r + ',' + r + ' 0 0 1 ' + r + ',' + r + ' v' + (h - 2 * r) + ' a' + r + ',' + r + ' 0 0 1 ' + -r + ',' + r + ' h' + -(w - 2 * r) + ' a' + r + ',' + r + ' 0 0 1 ' + -r + ',' + -r + ' v' + -(h - 2 * r) + ' a' + r + ',' + r + ' 0 0 1 ' + r + ',' + -r + ' Z'; }
function cross(x, y, a, b) { return 'M' + (x - a) + ',' + (y - b) + ' h' + 2 * a + ' v' + (b - a) + ' h' + (b - a) + ' v' + 2 * a + ' h' + -(b - a) + ' v' + (b - a) + ' h' + -2 * a + ' v' + -(b - a) + ' h' + -(b - a) + ' v' + -2 * a + ' h' + (b - a) + ' Z'; }
var MIR = 'translate(300,0) scale(-1,1)';

// ---- v3.7.0: bodies traced from product photos, keys at the photo positions (user Q11) ----
// Fixed constants from the approved mockup (assets data/bodies.json, ds-ext.json, labelfix.json, mock-board/padart.js).
var BODY = {
  dualsense: 'M150.0,30.0 C147.7,30.0 141.0,30.0 136.5,30.1 C131.9,30.2 127.2,30.4 122.6,30.6 C118.0,30.8 113.3,31.1 108.7,31.5 C104.1,31.9 99.5,32.5 94.9,33.0 C90.3,33.5 85.7,33.9 81.1,34.6 C76.6,35.4 72.0,36.3 67.6,37.5 C63.1,38.7 58.8,40.4 54.4,41.9 C50.0,43.5 45.3,44.4 41.3,46.6 C37.4,48.8 33.4,51.7 30.5,55.1 C27.7,58.6 26.1,63.2 24.3,67.5 C22.5,71.7 21.0,76.1 19.6,80.5 C18.1,84.9 16.8,89.4 15.6,93.8 C14.3,98.3 13.2,102.8 12.2,107.3 C11.1,111.8 10.1,116.3 9.3,120.9 C8.5,125.4 7.7,130.0 7.1,134.6 C6.5,139.2 6.1,143.8 5.7,148.4 C5.4,153.0 5.1,157.7 5.1,162.3 C5.1,166.9 5.2,171.6 5.6,176.2 C6.0,180.8 6.4,185.4 7.4,189.9 C8.4,194.4 9.5,199.1 11.7,203.1 C14.0,207.0 17.1,210.8 20.8,213.3 C24.5,215.8 29.5,217.8 33.8,218.0 C38.1,218.2 43.5,217.0 46.8,214.5 C50.2,212.0 52.0,206.9 54.1,202.8 C56.2,198.8 57.6,194.3 59.3,190.0 C61.0,185.7 62.4,181.2 64.3,177.0 C66.1,172.8 67.8,168.2 70.5,164.6 C73.3,161.0 76.7,157.4 80.7,155.4 C84.6,153.3 89.5,152.9 94.1,152.5 C98.7,152.1 103.4,152.8 108.0,152.9 C112.6,152.9 117.2,152.9 121.9,152.9 C126.5,152.9 131.1,152.9 135.8,152.9 C140.4,152.9 145.3,152.9 150.0,152.9 C154.7,152.9 159.6,152.9 164.2,152.9 C168.9,152.9 173.5,152.9 178.1,152.9 C182.8,152.9 187.4,152.9 192.0,152.9 C196.6,152.8 201.3,152.1 205.9,152.5 C210.5,152.9 215.4,153.3 219.3,155.4 C223.3,157.4 226.7,161.0 229.5,164.6 C232.2,168.2 233.9,172.8 235.7,177.0 C237.6,181.2 239.0,185.7 240.7,190.0 C242.4,194.3 243.8,198.8 245.9,202.8 C248.0,206.9 249.8,212.0 253.2,214.5 C256.5,217.0 261.9,218.2 266.2,218.0 C270.5,217.8 275.5,215.8 279.2,213.3 C282.9,210.8 286.0,207.0 288.3,203.1 C290.5,199.1 291.6,194.4 292.6,189.9 C293.6,185.4 294.0,180.8 294.4,176.2 C294.8,171.6 294.9,166.9 294.9,162.3 C294.9,157.7 294.6,153.0 294.3,148.4 C293.9,143.8 293.5,139.2 292.9,134.6 C292.3,130.0 291.5,125.4 290.7,120.9 C289.9,116.3 288.9,111.8 287.8,107.3 C286.8,102.8 285.7,98.3 284.4,93.8 C283.2,89.4 281.9,84.9 280.4,80.5 C279.0,76.1 277.5,71.7 275.7,67.5 C273.9,63.2 272.3,58.6 269.5,55.1 C266.6,51.7 262.6,48.8 258.7,46.6 C254.7,44.4 250.0,43.5 245.6,41.9 C241.2,40.4 236.9,38.7 232.4,37.5 C228.0,36.3 223.4,35.4 218.9,34.6 C214.3,33.9 209.7,33.5 205.1,33.0 C200.5,32.5 195.9,31.9 191.3,31.5 C186.7,31.1 182.0,30.8 177.4,30.6 C172.8,30.4 168.1,30.2 163.5,30.1 C159.0,30.0 152.3,30.0 150.0,30.0 Z',
  ns_pro: 'M150.0,28.0 C147.8,28.0 141.1,28.0 136.5,28.0 C132.0,28.1 127.3,28.2 122.6,28.2 C118.0,28.3 113.4,28.1 108.7,28.3 C104.1,28.5 99.5,28.7 94.9,29.4 C90.3,30.1 85.8,31.3 81.3,32.4 C76.8,33.5 72.4,34.8 67.9,36.1 C63.4,37.3 58.9,38.2 54.5,39.7 C50.1,41.2 45.4,42.4 41.6,44.9 C37.9,47.3 34.4,50.7 31.8,54.4 C29.2,58.1 27.5,62.7 26.0,67.0 C24.4,71.4 23.6,76.0 22.5,80.5 C21.5,85.0 20.6,89.5 19.7,94.1 C18.7,98.6 17.9,103.2 17.0,107.7 C16.2,112.3 15.3,116.8 14.6,121.4 C13.8,126.0 13.0,130.5 12.2,135.1 C11.5,139.7 10.7,144.3 10.0,148.8 C9.2,153.4 8.5,158.0 7.8,162.6 C7.2,167.2 6.4,171.7 6.0,176.4 C5.6,181.0 5.2,185.6 5.4,190.2 C5.6,194.8 5.9,199.6 7.4,203.9 C8.8,208.2 11.0,212.7 13.9,216.1 C16.9,219.4 21.1,222.6 25.3,223.9 C29.4,225.1 34.8,225.1 38.9,223.8 C43.1,222.4 46.9,219.0 50.1,215.8 C53.2,212.5 55.5,208.2 57.8,204.3 C60.2,200.3 62.1,196.0 64.3,191.9 C66.4,187.9 68.1,183.3 70.8,179.7 C73.6,176.0 76.7,172.2 80.6,170.0 C84.4,167.9 89.4,167.3 93.9,166.7 C98.5,166.1 103.2,166.6 107.8,166.6 C112.5,166.5 117.1,166.6 121.7,166.6 C126.4,166.6 130.9,166.6 135.6,166.6 C140.4,166.6 145.2,166.6 150.0,166.6 C154.8,166.6 159.6,166.6 164.4,166.6 C169.1,166.6 173.6,166.6 178.3,166.6 C182.9,166.6 187.5,166.5 192.2,166.6 C196.8,166.6 201.5,166.1 206.1,166.7 C210.6,167.3 215.6,167.9 219.4,170.0 C223.3,172.2 226.4,176.0 229.2,179.7 C231.9,183.3 233.6,187.9 235.7,191.9 C237.9,196.0 239.8,200.3 242.2,204.3 C244.5,208.2 246.8,212.5 249.9,215.8 C253.1,219.0 256.9,222.4 261.1,223.8 C265.2,225.1 270.6,225.1 274.7,223.9 C278.9,222.6 283.1,219.4 286.1,216.1 C289.0,212.7 291.2,208.2 292.6,203.9 C294.1,199.6 294.4,194.8 294.6,190.2 C294.8,185.6 294.4,181.0 294.0,176.4 C293.6,171.7 292.8,167.2 292.2,162.6 C291.5,158.0 290.8,153.4 290.0,148.8 C289.3,144.3 288.5,139.7 287.8,135.1 C287.0,130.5 286.2,126.0 285.4,121.4 C284.7,116.8 283.8,112.3 283.0,107.7 C282.1,103.2 281.3,98.6 280.3,94.1 C279.4,89.5 278.5,85.0 277.5,80.5 C276.4,76.0 275.6,71.4 274.0,67.0 C272.5,62.7 270.8,58.1 268.2,54.4 C265.6,50.7 262.1,47.3 258.4,44.9 C254.6,42.4 249.9,41.2 245.5,39.7 C241.1,38.2 236.6,37.3 232.1,36.1 C227.6,34.8 223.2,33.5 218.7,32.4 C214.2,31.3 209.7,30.1 205.1,29.4 C200.5,28.7 195.9,28.5 191.3,28.3 C186.6,28.1 182.0,28.3 177.4,28.2 C172.7,28.2 168.0,28.1 163.5,28.0 C158.9,28.0 152.2,28.0 150.0,28.0 Z',
  ns2_pro: 'M150.0,28.0 C147.7,28.0 141.0,28.1 136.4,28.2 C131.8,28.3 127.1,28.3 122.4,28.5 C117.8,28.8 113.1,28.9 108.5,29.5 C103.9,30.1 99.3,31.0 94.8,32.0 C90.3,33.0 85.8,34.4 81.3,35.6 C76.8,36.8 72.3,38.0 67.8,39.2 C63.3,40.5 58.8,41.4 54.3,42.9 C49.9,44.3 45.1,45.4 41.3,47.8 C37.5,50.2 34.0,53.6 31.4,57.3 C28.9,61.1 27.5,65.8 26.0,70.2 C24.5,74.6 23.6,79.2 22.5,83.7 C21.4,88.2 20.5,92.8 19.6,97.3 C18.7,101.9 17.9,106.5 17.1,111.1 C16.2,115.6 15.4,120.2 14.6,124.8 C13.8,129.4 13.0,134.0 12.2,138.6 C11.4,143.1 10.6,147.7 9.8,152.3 C9.1,156.9 8.4,161.5 7.7,166.1 C7.0,170.7 6.3,175.3 5.9,180.0 C5.5,184.6 5.0,189.3 5.2,193.9 C5.4,198.5 5.8,203.3 7.2,207.7 C8.6,212.0 10.7,216.6 13.6,219.9 C16.6,223.3 20.8,226.4 25.0,227.8 C29.1,229.2 34.5,229.3 38.7,228.1 C42.9,226.9 46.9,223.6 50.2,220.5 C53.5,217.3 55.9,213.2 58.4,209.2 C60.9,205.3 62.9,201.1 65.1,197.0 C67.3,192.9 68.9,188.3 71.4,184.5 C74.0,180.7 76.8,176.5 80.5,174.2 C84.3,171.8 89.3,171.1 93.8,170.4 C98.3,169.8 103.1,170.3 107.8,170.2 C112.4,170.2 117.1,170.2 121.7,170.2 C126.4,170.2 131.0,170.2 135.7,170.2 C140.4,170.2 145.2,170.2 150.0,170.2 C154.8,170.2 159.6,170.2 164.3,170.2 C169.0,170.2 173.6,170.2 178.3,170.2 C182.9,170.2 187.6,170.2 192.2,170.2 C196.9,170.3 201.7,169.8 206.2,170.4 C210.7,171.1 215.7,171.8 219.5,174.2 C223.2,176.5 226.0,180.7 228.6,184.5 C231.1,188.3 232.7,192.9 234.9,197.0 C237.1,201.1 239.1,205.3 241.6,209.2 C244.1,213.2 246.5,217.3 249.8,220.5 C253.1,223.6 257.1,226.9 261.3,228.1 C265.5,229.3 270.9,229.2 275.0,227.8 C279.2,226.4 283.4,223.3 286.4,219.9 C289.3,216.6 291.4,212.0 292.8,207.7 C294.2,203.3 294.6,198.5 294.8,193.9 C295.0,189.3 294.5,184.6 294.1,180.0 C293.7,175.3 293.0,170.7 292.3,166.1 C291.6,161.5 290.9,156.9 290.2,152.3 C289.4,147.7 288.6,143.1 287.8,138.6 C287.0,134.0 286.2,129.4 285.4,124.8 C284.6,120.2 283.8,115.6 282.9,111.1 C282.1,106.5 281.3,101.9 280.4,97.3 C279.5,92.8 278.6,88.2 277.5,83.7 C276.4,79.2 275.5,74.6 274.0,70.2 C272.5,65.8 271.1,61.1 268.6,57.3 C266.0,53.6 262.5,50.2 258.7,47.8 C254.9,45.4 250.1,44.3 245.7,42.9 C241.2,41.4 236.7,40.5 232.2,39.2 C227.7,38.0 223.2,36.8 218.7,35.6 C214.2,34.4 209.7,33.0 205.2,32.0 C200.7,31.0 196.1,30.1 191.5,29.5 C186.9,28.9 182.2,28.8 177.6,28.5 C172.9,28.3 168.2,28.3 163.6,28.2 C159.0,28.1 152.3,28.0 150.0,28.0 Z'
};
var DS_PANEL = 'M150.0,90.0 C143.0,90.0 116.3,89.2 108.0,90.0 C99.7,90.8 103.2,92.5 100.0,95.0 C96.8,97.5 92.3,101.7 89.0,105.0 C85.7,108.3 83.0,110.7 80.0,115.0 C77.0,119.3 73.8,125.2 71.0,131.0 C68.2,136.8 65.7,143.5 63.0,150.0 C60.3,156.5 57.8,163.0 55.0,170.0 C52.2,177.0 49.0,185.3 46.0,192.0 C43.0,198.7 39.2,206.0 37.0,210.0 C34.8,214.0 31.8,215.0 33.0,216.0 C34.2,217.0 41.3,217.4 44.4,215.9 C47.6,214.5 49.9,210.4 52.0,207.2 C54.1,203.9 55.4,200.1 56.9,196.4 C58.4,192.8 59.6,189.0 61.0,185.4 C62.5,181.7 63.8,177.9 65.4,174.4 C67.1,170.8 68.6,167.0 71.0,164.0 C73.4,160.9 76.3,157.9 79.6,156.0 C82.9,154.1 87.0,153.4 90.9,152.8 C94.7,152.2 98.8,152.6 102.7,152.6 C106.6,152.6 110.6,152.9 114.5,152.9 C118.5,152.9 122.4,152.9 126.3,152.9 C130.3,152.9 134.2,152.9 138.2,152.9 C142.1,152.9 146.1,152.9 150.0,152.9 C153.9,152.9 157.9,152.9 161.8,152.9 C165.8,152.9 169.7,152.9 173.7,152.9 C177.6,152.9 181.5,152.9 185.5,152.9 C189.4,152.9 193.4,152.6 197.3,152.6 C201.2,152.6 205.3,152.2 209.1,152.8 C213.0,153.4 217.1,154.1 220.4,156.0 C223.7,157.9 226.6,160.9 229.0,164.0 C231.4,167.0 232.9,170.8 234.6,174.4 C236.2,177.9 237.5,181.7 239.0,185.4 C240.4,189.0 241.6,192.8 243.1,196.4 C244.6,200.1 245.9,203.9 248.0,207.2 C250.1,210.4 252.4,214.5 255.6,215.9 C258.7,217.4 265.8,217.0 267.0,216.0 C268.2,215.0 265.2,214.0 263.0,210.0 C260.8,206.0 257.0,198.7 254.0,192.0 C251.0,185.3 247.8,177.0 245.0,170.0 C242.2,163.0 239.7,156.5 237.0,150.0 C234.3,143.5 231.8,136.8 229.0,131.0 C226.2,125.2 223.0,119.3 220.0,115.0 C217.0,110.7 214.3,108.3 211.0,105.0 C207.7,101.7 203.2,97.5 200.0,95.0 C196.8,92.5 200.3,90.8 192.0,90.0 C183.7,89.2 157.0,90.0 150.0,90.0 Z';
var NS_GRIP = 'M20.5,89.8 C20.2,91.6 19.1,96.9 18.4,100.4 C17.7,103.9 17.1,107.4 16.4,110.9 C15.8,114.5 15.2,118.0 14.6,121.5 C14.0,125.1 13.3,128.6 12.7,132.1 C12.1,135.6 11.6,139.2 11.0,142.7 C10.4,146.3 9.8,149.8 9.2,153.3 C8.7,156.9 8.1,160.4 7.6,164.0 C7.1,167.5 6.5,171.0 6.2,174.6 C5.8,178.2 5.5,181.7 5.4,185.3 C5.3,188.9 5.3,192.5 5.8,196.1 C6.3,199.6 7.0,203.2 8.3,206.5 C9.6,209.7 11.4,213.1 13.6,215.8 C15.9,218.4 18.8,221.0 21.9,222.5 C25.1,224.0 28.9,224.9 32.3,224.8 C35.8,224.7 39.5,223.7 42.6,222.1 C45.6,220.4 48.4,217.8 50.7,215.1 C53.1,212.5 54.8,209.3 56.7,206.2 C58.5,203.1 60.2,199.9 61.8,196.7 C63.5,193.6 64.9,190.3 66.6,187.1 C68.3,184.0 70.0,180.7 72.1,177.9 C74.3,175.2 81.5,176.0 79.8,170.5 C78.1,165.0 67.8,153.4 62.0,145.0 C56.2,136.6 50.3,127.5 45.0,120.0 C39.7,112.5 34.0,105.0 30.0,100.0 C26.0,95.0 22.5,91.7 21.0,90.0 Z';
var DS_EXT = { back: 'M33,58 C37,38 52,22 70,16.2 L82.20,14.77 Q84.20,14.52 84.50,16.52 L87.76,35.42 L58,40 C47,44 40,50 36,58 Z', front: 'M24.5,62 C29,48 44,36.5 64,32.8 L87.91,30.48 Q89.91,30.28 90.26,32.28 L91.74,41.06 Q92.09,43.06 90.09,43.26 L65,45.6 C53,48 44,55 38.5,64.5 Q36.5,68.5 31.5,67.5 Q23,65.5 24.5,62 Z' };
var NS2_PLATE = 'M150,33.6 L82.7,37 C66,38 54,40 46.5,43 C40,45.5 36,48.5 34,52 C31,58 29.5,68 29,80 C28.5,96 29.5,110 31,118 C33,129 37,140 44,150 C48.5,156 53,160 57,162 C61,165 65,167 70,167 L230,167 C235,167 239,165 243,162 C247,160 251.5,156 256,150 C263,140 267,129 269,118 C270.5,110 271.5,96 271,80 C270.5,68 269,58 266,52 C264,48.5 260,45.5 253.5,43 C246,40 234,38 217.3,37 Z';

// ---- L/R (left side; the right side is mirrored) ----
// The original contour shoulder (back: trigger, drawn before the body; front: bumper, after it), placed on the
// traced corner by the body fit (user Q13 / Q14): Switch Pro / Switch 2 Pro = shoulder A (the fit as is);
// DualSense = shoulder F (the fit moved by (-2, -2), and the extended paths of ds-ext.json whose bumper meets
// the touchpad). Labels: [x, y, rotate, cap height] in pad units, placed inside each visible key face.
var CONTOUR = {
  back: 'M32,62 C36,36 56,18 86,14 L97,13 Q104,12.5 104,19 L104,32 L60,40 C46,45 38,53 35,64 Z',
  front: 'M19,84 C22,56 42,37 70,32 L98,29.5 Q104,29 104,35.5 Q104,42 98,42.5 L72,45 C53,49 39,63 34,86 Q33,92 26.5,91 Q18.5,90 19,84 Z'
};
var SHOULDER = {
  dualsense: { fit: { s: 0.9893, tx: 1.11 - 2, ty: -1.41 - 2 }, back: DS_EXT.back, front: DS_EXT.front, role: 'bump',
    trigT: ['L2', 'R2', 74.3, 18.9, -6, 6], bumpT: ['L1', 'R1', 70.5, 34.6, -5, 5], trigA: [50, 30], bumpA: [31, 61] },
  ns_pro: { fit: { s: 0.9555, tx: 4.63, ty: -2.79 }, back: CONTOUR.back, front: CONTOUR.front, role: 'bump',
    trigT: ['ZL', 'ZR', 85.8475, 18.231, 0, 6], bumpT: ['L', 'R', 54.4, 40.8, -11, 5.5], trigA: [60, 25], bumpA: [27, 72] },
  ns2_pro: { fit: { s: 0.9507, tx: 5.13, ty: -1.14 }, back: CONTOUR.back, front: CONTOUR.front, role: 'bumpLite',
    trigT: ['ZL', 'ZR', 86.4, 19.8, 0, 6], bumpT: ['L', 'R', 84, 34.2, -5, 5], trigA: [60, 25], bumpA: [27, 72] }
};
function fitOf(pad) { var f = SHOULDER[pad].fit; return { tf: 'translate(' + f.tx + ' ' + f.ty + ') scale(' + f.s + ')', at: function (q) { return [f.tx + f.s * q[0], f.ty + f.s * q[1]]; } }; }

// Key positions on the photo art (pad units). Switch pads: ls / dp / rs sticks and d-pad, mi / pl minus and
// plus, cap / home, fc the face diamond centre.
var NS_KEYS = {
  ns_pro: { ls: [70, 84], dp: [105, 125], rs: [188, 125], mi: [113, 63.5], pl: [186, 63.5], cap: [129, 85], home: [171, 85], fc: [227, 85] },
  ns2_pro: { ls: [69, 89], dp: [106, 132], rs: [185, 129], mi: [114, 66], pl: [186, 66], cap: [132, 88], home: [168, 88], fc: [224.5, 87] }
};

function model(pad) {
  var P = [], Gl = [], S = SHOULDER[pad], F = fitOf(pad), ds = pad === 'dualsense', ns2 = pad === 'ns2_pro';
  function add(d, role, tf) { P.push({ d: d, role: role, tf: tf || '' }); }
  function both(d, role, tf) { add(d, role, tf); add(d, role, MIR + (tf ? ' ' + tf : '')); }
  function stick(q) { add(circ(q[0], q[1], 20), 'well'); add(circ(q[0], q[1], 13.5), 'cap'); add(circ(q[0], q[1], 8.5), 'capTop'); }
  function face(cx, cy, sp, sym) { [[cx, cy - sp], [cx + sp, cy], [cx, cy + sp], [cx - sp, cy]].forEach(function (q, i) { add(circ(q[0], q[1], 9), 'face'); Gl.push({ kind: sym[i].length === 1 ? 'text' : sym[i], t: sym[i], x: q[0], y: q[1], role: 'faceT' }); }); }
  function label(T, role) {
    Gl.push({ kind: 'text', t: T[0], x: T[2], y: T[3], rot: T[4], cap: T[5], role: role }, { kind: 'text', t: T[1], x: 300 - T[2], y: T[3], rot: -T[4], cap: T[5], role: role });
  }
  if (ns2) both(rr(40, 199, 24, 20, 6), 'back');   // GL / GR, peeking out under the grips
  both(S.back, 'trig', F.tf);
  add(BODY[pad], 'body');
  if (ds) {
    add(DS_PANEL, 'panel');
    add('M93,32 H207 Q210,32 209.5,35 L201.5,82 Q200,90 192,90 H108 Q100,90 98.5,82 L90.5,35 Q90,32 93,32 Z', 'tpad');
  } else if (ns2) add(NS2_PLATE, 'plateDark');
  else { both(NS_GRIP, 'grip'); add('M101,40.5 H199', 'seam'); }
  both(S.front, S.role, F.tf);
  label(S.trigT, 'trigT');
  label(S.bumpT, S.role === 'bumpLite' ? 'bumpLiteT' : 'bumpT');
  if (ds) {
    add(rr(78, 44, 8, 13, 4), 'small'); add(rr(214, 44, 8, 13, 4), 'small');   // Create / Options, at the photo position
    var dk = 'M56,60 H65 Q67,60 67,62 V68.5 L61.5,73.5 Q60.5,74.4 59.5,73.5 L54,68.5 V62 Q54,60 56,60 Z';
    [0, 90, 180, 270].forEach(function (r) { add(dk, 'dkey', 'rotate(' + r + ' 60.5 83)'); });
    stick([104.5, 121]); stick([195.5, 121]);
    for (var gx = 0; gx < 4; gx++) for (var gy = 0; gy < 2; gy++) add(circ(142.5 + gx * 5, 99 + gy * 4.5, 1.3), 'grille');
    add(circ(150, 119, 7), 'small'); Gl.push({ kind: 'ps', x: 150, y: 119, role: 'smallT' });
    add(rr(143, 136, 14, 4, 2), 'small');   // mute
    face(240, 83, 21, ['tri', 'ring', 'cross', 'sq']);   // a 1-letter id would be drawn as text
  } else {
    var n = NS_KEYS[pad];
    add(cross(n.dp[0], n.dp[1], 6.5, 16), 'dcross');
    stick(n.ls); stick(n.rs);
    add(circ(n.mi[0], n.mi[1], 6.5), 'small'); add('M' + (n.mi[0] - 3.5) + ',' + n.mi[1] + ' h7', 'seam');
    add(circ(n.pl[0], n.pl[1], 6.5), 'small'); add('M' + (n.pl[0] - 3.5) + ',' + n.pl[1] + ' h7 M' + n.pl[0] + ',' + (n.pl[1] - 3.5) + ' v7', 'seam');
    add(rr(n.cap[0] - 6, n.cap[1] - 6, 12, 12, 3), 'small'); Gl.push({ kind: 'dot', x: n.cap[0], y: n.cap[1], role: 'smallT' });
    add(circ(n.home[0], n.home[1], 7), 'small'); Gl.push({ kind: 'home', x: n.home[0], y: n.home[1], role: 'smallT' });
    if (ns2) { add(rr(144, 149, 12, 12, 3), 'small'); Gl.push({ kind: 'c', x: 150, y: 155, role: 'smallT' }); }
    face(n.fc[0], n.fc[1], 21, ['X', 'A', 'B', 'Y']);
  }
  return { parts: P, glyphs: Gl };
}

// Line end points on the pad art, keyed by route id (pad units; left side only for L2 / L1 / GL, the board
// mirrors ZR / R1 / GR). Triggers end on the left of their visible part, clear of the label; the shoulder
// points go through the shoulder's own fit.
export function padAnchors(pad) {
  var S = SHOULDER[pad];
  if (!S) return null;
  var F = fitOf(pad), a = { zl: F.at(S.trigA), l1: F.at(S.bumpA) };
  if (pad === 'dualsense') return Object.assign(a, { ls: [104.5, 121], dpad: [60.5, 83], rs: [195.5, 121], face: [240, 83], share: [82, 50.5], opt: [218, 50.5], ps: [150, 119], tp: [150, 61] });
  var n = NS_KEYS[pad];
  Object.assign(a, { ls: n.ls, dpad: n.dp, rs: n.rs, minus: n.mi, plus: n.pl, cap: n.cap, home: n.home, face: n.fc });
  if (pad === 'ns2_pro') Object.assign(a, { c: [150, 155], gl: [61, 212] });
  return a;
}

function pixGlyph(rows, cx, cy, c) {
  var w = rows[0].length, h = rows.length, x0 = cx - w / 2, y0 = cy - h / 2, d = '';
  rows.forEach(function (r, yy) { for (var xx = 0; xx < w; xx++) if (r[xx] === '#') d += 'M' + (x0 + xx) + ',' + (y0 + yy) + 'h1v1h-1Z'; });
  return '<path d="' + d + '" fill="' + c + '" shape-rendering="crispEdges"/>';
}
// Labels on the controller are drawn as stroke letters (cap height 7 units): text glyphs from the pixel
// font are not centred on their ink and blur off the pixel grid, so A/B/X/Y and L/R read off-centre.
var VL = {
  A: [6, 'M0,7 L3,0 L6,7 M1.3,4.4 H4.7'], B: [5, 'M0,0 V7 H3 Q5,7 5,5.1 Q5,3.5 3,3.5 H0 M0,0 H2.7 Q4.6,0 4.6,1.75 Q4.6,3.5 2.7,3.5'],
  X: [5, 'M0,0 L5,7 M5,0 L0,7'], Y: [6, 'M0,0 L3,3.4 L6,0 M3,3.4 V7'], L: [4, 'M0,0 V7 H4'], R: [5, 'M0,7 V0 H3 Q5,0 5,2 Q5,4 3,4 H0 M3,4 L5,7'],
  Z: [5, 'M0,0 H5 L0,7 H5'], P: [5, 'M0,7 V0 H3 Q5,0 5,2 Q5,4 3,4 H0'], S: [5, 'M5,1 Q4.4,0 2.6,0 Q0.2,0 0.2,1.8 Q0.2,3.2 2.5,3.5 Q5,3.8 5,5.3 Q5,7 2.5,7 Q0.6,7 0,6'],
  '1': [3, 'M0,1.5 L1.8,0 V7 M0,7 H3.6'], '2': [5, 'M0,1.8 Q0,0 2.5,0 Q5,0 5,2 Q5,3.4 3.6,4.4 L0,7 H5']
};
function vtext(t, cx, cy, cap, c, sw, rot) {
  var k = cap / 7, gap = 1.8, w = -gap, d = '';
  t.split('').forEach(function (ch) { w += VL[ch][0] + gap; });
  var x = 0;
  t.split('').forEach(function (ch) { d += '<path d="' + VL[ch][1] + '" transform="translate(' + x + ' 0)"/>'; x += VL[ch][0] + gap; });
  return '<g transform="' + (rot ? 'rotate(' + rot + ' ' + cx + ' ' + cy + ') ' : '') + 'translate(' + (cx - w * k / 2) + ' ' + (cy - cap / 2) + ') scale(' + k + ')" fill="none" stroke="' + c + '" stroke-width="' + (sw / k) + '" stroke-linecap="round" stroke-linejoin="round">' + d + '</g>';
}
function glyphSvg(g, c) {
  var x = g.x, y = g.y;
  if (g.kind === 'text' && /^[ABXYLRZ12]+$/.test(g.t)) return vtext(g.t, x, y, g.role === 'faceT' ? 7 : (g.cap || 6), c, g.role === 'faceT' ? 1.6 : 1.4 * (g.cap || 6) / 6, g.rot);
  if (g.kind === 'text') {
    var fs = g.role === 'faceT' ? 10 : 9;
    return '<text x="' + x + '" y="' + (y + fs * 0.36) + '"' + (g.rot ? ' transform="rotate(' + g.rot + ' ' + x + ' ' + y + ')"' : '') + ' text-anchor="middle" font-size="' + fs + '" font-family="Cubic11,Microsoft JhengHei,sans-serif" fill="' + c + '">' + g.t + '</text>';
  }
  if (g.kind === 'tri') return '<path d="M' + x + ',' + (y - 4.5) + ' L' + (x + 4.5) + ',' + (y + 3.5) + ' L' + (x - 4.5) + ',' + (y + 3.5) + ' Z" fill="none" stroke="' + c + '" stroke-width="1.6" stroke-linejoin="round"/>';
  if (g.kind === 'ring') return '<circle cx="' + x + '" cy="' + y + '" r="4" fill="none" stroke="' + c + '" stroke-width="1.6"/>';
  if (g.kind === 'cross') return '<path d="M' + (x - 3.6) + ',' + (y - 3.6) + ' L' + (x + 3.6) + ',' + (y + 3.6) + ' M' + (x + 3.6) + ',' + (y - 3.6) + ' L' + (x - 3.6) + ',' + (y + 3.6) + '" stroke="' + c + '" stroke-width="1.6" stroke-linecap="round"/>';
  if (g.kind === 'sq') return '<rect x="' + (x - 3.6) + '" y="' + (y - 3.6) + '" width="7.2" height="7.2" fill="none" stroke="' + c + '" stroke-width="1.6"/>';
  if (g.kind === 'dot') return '<circle cx="' + x + '" cy="' + y + '" r="2.6" fill="' + c + '"/>';
  // Home = pixel house (H3), PS = the letters PS in pixels (P4), user picks 2026-10-13. Both are 8 px
  // bitmaps on whole pixels, centred on the r7 button with clearance from its rim, 1 px strokes like the font.
  if (g.kind === 'home') return '<g transform="translate(' + x + ' ' + y + ')" fill="none" stroke="' + c + '" stroke-width="1.9" stroke-linejoin="round" stroke-linecap="round"><path d="M-3.6,-0.4 L0,-3.8 L3.6,-0.4"/><path d="M-2.4,-1.2 V3.4 H2.4 V-1.2"/><rect x="-0.8" y="0.9" width="1.6" height="2.5" fill="' + c + '" stroke="none"/></g>';
  if (g.kind === 'c') return '<path d="M' + (x + 2.2) + ',' + (y - 2.4) + ' A3.2,3.2 0 1 0 ' + (x + 2.2) + ',' + (y + 2.4) + '" fill="none" stroke="' + c + '" stroke-width="1.5"/>';
  if (g.kind === 'ps') return vtext('PS', x, y, 4.6, c, 1.5);  // smaller so it clears the r7 rim
  return '';
}

// Outline width per part (0 = none).
var WIDTH = { panel: 2.5, grip: 2.5, plateDark: 2, seam: 2, grille: 0, bumpLite: 3.5, body: 5, tpad: 3.5, trig: 3.5, bump: 3.5, well: 3.5, cap: 3, capTop: 0, dcross: 3, dkey: 3, back: 3, face: 3 };
// The board pad is line work like the MapleStory controller (v3.3.0): navy ink, the body shows the panel
// through, slate keys, no colour, no drop shadow. The app / tray icons are scripts/icon-art.mjs.
var INK = '#2d4359';
var LINE = { panel: '#8c959d', grip: '#a2aab1', plateDark: '#abb3b9', seam: 'none', grille: INK, bumpLite: '#d4d9dd', body: '#b2bac0', tpad: '#b2bac0', trig: '#7b8188', back: '#7b8188', bump: '#8f969d', well: '#b2bac0', cap: '#79838f', capTop: '#8a939d', dcross: '#79838f', dkey: '#79838f', face: '#79838f' };
function boardFill(role) { var w = role in WIDTH ? WIDTH[role] : 2.5; return [LINE[role] || '#79838f', w ? Math.min(w, 3) : 0]; }

// Every part carries its role and every glyph its role and anchor (data-*), which web-smoke measures.
function padArt(pad) {
  var M = model(pad), h = '<svg viewBox="-30 -30 360 270" aria-hidden="true">';
  M.parts.forEach(function (p) {
    var f = boardFill(p.role);
    h += '<path data-role="' + p.role + '" d="' + p.d + '"' + (p.tf ? ' transform="' + p.tf + '"' : '') + ' fill="' + f[0] + '"' + (f[1] ? ' stroke="' + INK + '" stroke-width="' + f[1] + '" stroke-linejoin="round"' : '') + '/>';
  });
  // Switch 2 Pro's light bumpers take dark labels.
  h += M.glyphs.map(function (g) { return '<g data-glyph="' + g.role + '" data-x="' + g.x + '" data-y="' + g.y + '" data-rot="' + (g.rot || 0) + '">' + glyphSvg(g, g.role === 'smallT' || g.role === 'bumpLiteT' ? INK : '#ffffff') + '</g>'; }).join('');
  return h + '<g class="ancg"></g></svg>';
}


// The pad drawing, positioned on the 980×640 board.
export function padSvg(pad) {
  return padArt(pad).replace('<svg viewBox="-30 -30 360 270" aria-hidden="true">',
    '<svg class="pad" style="left:340px;top:150px;overflow:visible" width="300" height="210" viewBox="0 0 300 210" aria-hidden="true">');
}
