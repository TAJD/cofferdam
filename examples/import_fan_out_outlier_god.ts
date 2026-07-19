// Fixture for Design.ImportFanOutOutlier.
// Imports every leaf file — a fan-out outlier vs. the 14 leaves' fan-out of 0.
import { leaf_a } from "./import_fan_out_outlier_leaf_a";
import { leaf_b } from "./import_fan_out_outlier_leaf_b";
import { leaf_c } from "./import_fan_out_outlier_leaf_c";
import { leaf_d } from "./import_fan_out_outlier_leaf_d";
import { leaf_e } from "./import_fan_out_outlier_leaf_e";
import { leaf_f } from "./import_fan_out_outlier_leaf_f";
import { leaf_g } from "./import_fan_out_outlier_leaf_g";
import { leaf_h } from "./import_fan_out_outlier_leaf_h";
import { leaf_i } from "./import_fan_out_outlier_leaf_i";
import { leaf_j } from "./import_fan_out_outlier_leaf_j";
import { leaf_k } from "./import_fan_out_outlier_leaf_k";
import { leaf_l } from "./import_fan_out_outlier_leaf_l";
import { leaf_m } from "./import_fan_out_outlier_leaf_m";
import { leaf_n } from "./import_fan_out_outlier_leaf_n";

export function useAll(): number {
  const values = [
    leaf_a(),
    leaf_b(),
    leaf_c(),
    leaf_d(),
    leaf_e(),
    leaf_f(),
    leaf_g(),
    leaf_h(),
    leaf_i(),
    leaf_j(),
    leaf_k(),
    leaf_l(),
    leaf_m(),
    leaf_n(),
  ];
  return values.reduce((sum, n) => sum + n, 0);
}
