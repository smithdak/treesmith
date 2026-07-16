using System.Web.Mvc;

namespace Sample.Controllers
{
    public class NavBarController : Controller
    {
        public ActionResult Index()
        {
            return View("~/Views/NavBar.cshtml");
        }
    }
}
