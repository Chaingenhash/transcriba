const langs = document.querySelector<HTMLSelectElement>("#lang")!;
for (const [code, label] of [["pt", "Português"], ["en", "English"], ["auto", "Detetar automaticamente"]]) {
  const opt = document.createElement("option");
  opt.value = code;
  opt.textContent = label;
  langs.append(opt);
}
langs.value = "pt";
