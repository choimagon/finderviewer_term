"""Create disposable files for mouse and file-operation demonstrations."""
from pathlib import Path
import json
import tempfile
import zipfile


def create_demo() -> tuple[Path, Path]:
    temporary=Path(tempfile.mkdtemp(prefix="finderview-terminal-demo-"))
    root=temporary/"터미널 테스트"
    root.mkdir()
    (root/"01_논문"/"리비전"/"세 번째").mkdir(parents=True)
    (root/"02_대상").mkdir()
    (root/"03_빈 폴더").mkdir()
    for name,content in [("README.md","# FinderView Terminal\n실제 문서가 아닌 임시 테스트 파일입니다.\n"),("이동할 파일.txt","move test 한글\n"),("복사할 파일.txt","copy test 한글\n"),("삭제할 파일.txt","trash test\n"),(".숨김파일","hidden\n")]:
        (root/name).write_text(content,encoding="utf8")
    with zipfile.ZipFile(root/"원고.docx","w") as doc:
        doc.writestr("[Content_Types].xml",'<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>')
        doc.writestr("_rels/.rels",'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>')
        doc.writestr("word/document.xml",'<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>FinderView 임시 테스트 문서</w:t></w:r></w:p></w:body></w:document>')
    return root,temporary/"events.jsonl"


if __name__=="__main__":
    root,log=create_demo()
    print(json.dumps({"root":str(root),"event_log":str(log)},ensure_ascii=False))
